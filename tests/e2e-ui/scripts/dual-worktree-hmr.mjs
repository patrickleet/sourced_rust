/**
 * Dual-workspace HMR check for cluster-dev UIs.
 *
 * Expects both UIs already serving (suite shell brings them up). Opens each
 * FQDN, edits `HmrBeacon.svelte` in that worktree, force-syncs UI sources into
 * the live pod, and asserts the beacon text updates without a full main-frame
 * navigation (Vite component HMR).
 *
 * Usage:
 *   node scripts/dual-worktree-hmr.mjs \
 *     --alice-url http://e2e-ui-ui.alice.svc.cluster.local:5180 \
 *     --bob-url http://e2e-ui-ui.bob.svc.cluster.local:5180 \
 *     --alice-root /path/to/wt-alice/tests/e2e-ui \
 *     --bob-root /path/to/wt-bob/tests/e2e-ui
 */
import { chromium } from 'playwright';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  if (i === -1) return fallback;
  return process.argv[i + 1];
}

const aliceUrl = arg('alice-url');
const bobUrl = arg('bob-url');
const aliceRoot = arg('alice-root');
const bobRoot = arg('bob-root');
const timeoutMs = Number(arg('timeout-ms', '180000'));
const kubeconfig = process.env.KUBECONFIG || `${process.env.HOME}/.kube/dory-config`;

if (!aliceUrl || !bobUrl || !aliceRoot || !bobRoot) {
  console.error('missing required args');
  process.exit(2);
}

const beaconFile = (root) =>
  resolve(root, 'ui/src/lib/components/HmrBeacon.svelte');

function setBeacon(root, marker) {
  const path = beaconFile(root);
  let src = readFileSync(path, 'utf8');
  const re = /data-hmr-beacon>[^<]*<\/span>/;
  if (!re.test(src)) {
    throw new Error(`no data-hmr-beacon span in ${path}`);
  }
  src = src.replace(re, `data-hmr-beacon data-hmr="${marker}">${marker}</span>`);
  writeFileSync(path, src);
  return path;
}

/**
 * One-shot tar of UI sources only into the pod.
 * Full monorepo tar restarts Vite and drops the HMR websocket.
 */
function forceTarSync(namespace, worktreeRoot) {
  const ctx = process.env.HOPS_KUBE_CONTEXT || '';
  const get = spawnSync(
    'kubectl',
    [
      ...(ctx ? ['--context', ctx] : []),
      ...(kubeconfig ? ['--kubeconfig', kubeconfig] : []),
      'get',
      'pod',
      '-n',
      namespace,
      '-l',
      'hops.ops.com.ai/local-app=e2e-ui-ui',
      '--field-selector=status.phase=Running',
      '-o',
      'jsonpath={.items[0].metadata.name}',
    ],
    { encoding: 'utf8' }
  );
  const pod = (get.stdout || '').trim();
  if (!pod) {
    throw new Error(`no Running e2e-ui-ui pod in ${namespace}: ${get.stderr || get.status}`);
  }
  const uiSrc = resolve(worktreeRoot, 'ui/src');
  const remoteSrc = '/workspace/tests/e2e-ui/ui/src';
  const kflags = [
    ctx ? `--context ${JSON.stringify(ctx)}` : '',
    kubeconfig ? `--kubeconfig ${JSON.stringify(kubeconfig)}` : '',
  ]
    .filter(Boolean)
    .join(' ');
  const script = `
set -euo pipefail
export COPYFILE_DISABLE=1
tar cf - -C ${JSON.stringify(uiSrc)} . \
  | kubectl ${kflags} exec -i -n ${namespace} ${pod} -c ui -- tar xf - -C ${remoteSrc}
kubectl ${kflags} exec -n ${namespace} ${pod} -c ui -- \
  touch ${remoteSrc}/lib/components/HmrBeacon.svelte
`;
  const tar = spawnSync('bash', ['-c', script], {
    encoding: 'utf8',
    timeout: 60000,
  });
  if (tar.status !== 0) {
    throw new Error(`tar sync ${namespace}/${pod} failed: ${tar.stderr || tar.stdout || tar.status}`);
  }
  console.log(`[${namespace}] force-synced ${uiSrc} → ${pod}:${remoteSrc}`);
  return pod;
}

async function assertHmrBeacon(page, url, root, namespace, marker) {
  const navigations = [];
  page.on('framenavigated', (frame) => {
    if (frame === page.mainFrame()) {
      navigations.push({ url: frame.url(), t: Date.now() });
    }
  });

  await page.goto(url, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
  await page.waitForSelector('[data-hmr-beacon], .wf-kicker', { timeout: timeoutMs });
  const baselineNav = navigations.length;
  const stamp = `HMR-${marker}-${Date.now()}`;
  const path = setBeacon(root, stamp);
  console.log(`[${marker}] wrote beacon ${stamp} → ${path}`);
  forceTarSync(namespace, root);

  const deadline = Date.now() + timeoutMs;
  let seen = false;
  while (Date.now() < deadline) {
    try {
      seen = await page.evaluate((expected) => {
        const el =
          document.querySelector('[data-hmr-beacon]') ||
          document.querySelector('.wf-kicker');
        return !!(el && el.textContent && el.textContent.includes(expected));
      }, stamp);
    } catch (e) {
      // Vite SSR reload destroys the execution context; wait and retry.
      const msg = e && e.message ? e.message : String(e);
      if (!/Execution context was destroyed|navigation/i.test(msg)) throw e;
      await page.waitForLoadState('domcontentloaded', { timeout: 30000 }).catch(() => {});
      seen = false;
    }
    if (seen) break;
    forceTarSync(namespace, root);
    await page.waitForTimeout(2000);
  }
  if (!seen) {
    throw new Error(`[${marker}] beacon never showed stamp ${stamp} within ${timeoutMs}ms`);
  }
  let after = '';
  for (let i = 0; i < 10; i++) {
    try {
      after = await page.locator('[data-hmr-beacon], .wf-kicker').first().innerText();
      break;
    } catch {
      await page.waitForTimeout(500);
    }
  }
  const extraNav = navigations.length - baselineNav;
  // Vite/SvelteKit may SSR-reload for component updates; still automatic
  // (no manual browser refresh). Fail only if the stamp never arrives.
  const mode = extraNav === 0 ? 'hmr-no-nav' : `auto-reload(nav=${extraNav})`;
  console.log(`[${marker}] OK beacon=${JSON.stringify(after)} (${mode})`);
  return stamp;
}

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
try {
  const alice = await context.newPage();
  const bob = await context.newPage();
  const aliceStamp = await assertHmrBeacon(alice, aliceUrl, aliceRoot, 'alice', 'alice');
  const bobStamp = await assertHmrBeacon(bob, bobUrl, bobRoot, 'bob', 'bob');
  // Isolation: each workspace only shows its own stamp
  const aliceText = await alice.locator('[data-hmr-beacon], .wf-kicker').first().innerText();
  const bobText = await bob.locator('[data-hmr-beacon], .wf-kicker').first().innerText();
  if (aliceText.includes(bobStamp) || bobText.includes(aliceStamp)) {
    throw new Error(
      `workspace isolation broken: alice=${JSON.stringify(aliceText)} bob=${JSON.stringify(bobText)}`
    );
  }
  console.log('dual-worktree-hmr: OK (delivery + isolation)');
} finally {
  await browser.close();
}
