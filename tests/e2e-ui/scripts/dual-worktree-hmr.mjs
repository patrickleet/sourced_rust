/**
 * Dual-workspace HMR check for cluster-dev UIs.
 *
 * Expects both UIs already serving (suite shell brings them up). Opens each
 * FQDN, edits `HmrBeacon.svelte` in that worktree, copies **only that file**
 * into the live pod (full-tree tar causes Vite SSR reload / full navigation),
 * and asserts the beacon updates with **zero** main-frame navigations.
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
const timeoutMs = Number(arg('timeout-ms', '120000'));
const kubeconfig = process.env.KUBECONFIG || `${process.env.HOME}/.kube/dory-config`;

if (!aliceUrl || !bobUrl || !aliceRoot || !bobRoot) {
  console.error('missing required args');
  process.exit(2);
}

const beaconRel = 'ui/src/lib/components/HmrBeacon.svelte';
const remoteBeacon = '/workspace/tests/e2e-ui/ui/src/lib/components/HmrBeacon.svelte';

const beaconFile = (root) => resolve(root, beaconRel);

function setBeacon(root, marker) {
  const path = beaconFile(root);
  let src = readFileSync(path, 'utf8');
  // Match with or without existing data-hmr attribute.
  const re =
    /(<span class="wf-kicker" data-hmr-beacon)(?:\s+data-hmr="[^"]*")?>([^<]*)(<\/span>)/;
  if (!re.test(src)) {
    throw new Error(`no data-hmr-beacon span in ${path}`);
  }
  src = src.replace(re, `$1 data-hmr="${marker}">${marker}$3`);
  writeFileSync(path, src);
  return path;
}

function kubectlBase() {
  const ctx = process.env.HOPS_KUBE_CONTEXT || '';
  return [
    'kubectl',
    ...(ctx ? ['--context', ctx] : []),
    ...(kubeconfig ? ['--kubeconfig', kubeconfig] : []),
  ];
}

function runningUiPod(namespace) {
  const base = kubectlBase();
  const get = spawnSync(
    base[0],
    [
      ...base.slice(1),
      'get',
      'pod',
      '-n',
      namespace,
      '-l',
      'hops.ops.com.ai/local-app=e2e-ui-api',
      '--field-selector=status.phase=Running',
      '-o',
      'jsonpath={.items[0].metadata.name}',
    ],
    { encoding: 'utf8' }
  );
  const pod = (get.stdout || '').trim();
  if (!pod) {
    throw new Error(`no Running e2e-ui application pod in ${namespace}: ${get.stderr || get.status}`);
  }
  return pod;
}

/**
 * Pause hops continuous full-tree tar watchers. Full monorepo re-tar after an
 * edit rewrites many files and forces Vite SSR page reload (full navigation),
 * which is not the HMR path under test.
 */
function pauseDeliveryWatchers() {
  // Match the multi-app tar watch script body hops spawns (sync_one / needs_marker).
  const out = spawnSync(
    'bash',
    [
      '-c',
      `pgrep -f 'sync_one\\(\\)|needs_marker\\(\\)' 2>/dev/null || true`,
    ],
    { encoding: 'utf8' }
  );
  const pids = (out.stdout || '')
    .split(/\s+/)
    .map((s) => s.trim())
    .filter((s) => /^\d+$/.test(s));
  for (const pid of pids) {
    spawnSync('kill', [pid], { encoding: 'utf8' });
    console.log(`paused delivery watcher pid=${pid}`);
  }
}

/**
 * Single-file delivery: host worktree beacon → pod path Vite watches.
 * Avoid full-tree tar (causes SSR reload / full navigation).
 */
function forceBeaconSync(namespace, worktreeRoot) {
  const pod = runningUiPod(namespace);
  const hostPath = beaconFile(worktreeRoot);
  const base = kubectlBase();
  // kubectl cp does not support -c for all versions; use tar of one file.
  const script = `
set -euo pipefail
export COPYFILE_DISABLE=1
tar cf - -C ${JSON.stringify(resolve(worktreeRoot, 'ui/src/lib/components'))} HmrBeacon.svelte \
  | ${base.join(' ')} exec -i -n ${namespace} ${pod} -c application -- \
    tar xf - -C /workspace/tests/e2e-ui/ui/src/lib/components
${base.join(' ')} exec -n ${namespace} ${pod} -c application -- touch ${remoteBeacon}
`;
  const tar = spawnSync('bash', ['-c', script], {
    encoding: 'utf8',
    timeout: 30000,
  });
  if (tar.status !== 0) {
    throw new Error(
      `beacon sync ${namespace}/${pod} failed: ${tar.stderr || tar.stdout || tar.status}`
    );
  }
  console.log(`[${namespace}] single-file sync ${hostPath} → ${pod}:${remoteBeacon}`);
  return pod;
}

async function assertHmrBeacon(page, url, root, namespace, marker) {
  const navigations = [];
  page.on('framenavigated', (frame) => {
    if (frame === page.mainFrame()) {
      navigations.push({ url: frame.url(), t: Date.now() });
    }
  });

  await page.goto(url, { waitUntil: 'networkidle', timeout: timeoutMs });
  await page.waitForSelector('[data-hmr-beacon]', { timeout: timeoutMs });
  // Give Vite HMR websocket time to connect before we edit.
  await page.waitForTimeout(1500);
  const baselineNav = navigations.length;

  const stamp = `HMR-${marker}-${Date.now()}`;
  const path = setBeacon(root, stamp);
  console.log(`[${marker}] wrote beacon ${stamp} → ${path}`);
  forceBeaconSync(namespace, root);

  const deadline = Date.now() + timeoutMs;
  let seen = false;
  while (Date.now() < deadline) {
    // Fail immediately if a full navigation already happened after the edit.
    const extraSoFar = navigations.length - baselineNav;
    if (extraSoFar > 0) {
      throw new Error(
        `[${marker}] full navigation during beacon update (extra=${extraSoFar}); HMR required (hmr-no-nav)`
      );
    }
    try {
      // CSS may text-transform: uppercase the kicker; compare case-insensitively.
      seen = await page.evaluate((expected) => {
        const el = document.querySelector('[data-hmr-beacon]');
        if (!el) return false;
        const t = (el.textContent || el.innerText || '').toUpperCase();
        return t.includes(String(expected).toUpperCase());
      }, stamp);
    } catch (e) {
      const msg = e && e.message ? e.message : String(e);
      // Context destroyed = navigation/reload — fail (do not swallow).
      if (/Execution context was destroyed|navigation/i.test(msg)) {
        throw new Error(
          `[${marker}] page reloaded during HMR wait (context destroyed): ${msg}`
        );
      }
      throw e;
    }
    if (seen) break;
    forceBeaconSync(namespace, root);
    await page.waitForTimeout(1000);
  }
  if (!seen) {
    throw new Error(`[${marker}] beacon never showed stamp ${stamp} within ${timeoutMs}ms`);
  }

  const after = await page.locator('[data-hmr-beacon]').innerText();
  const extraNav = navigations.length - baselineNav;
  if (extraNav > 0) {
    throw new Error(
      `[${marker}] full navigation during beacon update (extra=${extraNav}); text=${JSON.stringify(after)}`
    );
  }
  if (!after.toUpperCase().includes(stamp.toUpperCase())) {
    throw new Error(`[${marker}] beacon missing stamp: ${after}`);
  }
  console.log(`[${marker}] OK beacon=${JSON.stringify(after)} (hmr-no-nav)`);
  return stamp;
}

// Continuous full-tree delivery watchers fight single-file HMR (mass rewrite → SSR reload).
pauseDeliveryWatchers();

const browser = await chromium.launch({ headless: true });
const context = await browser.newContext();
try {
  const alice = await context.newPage();
  const bob = await context.newPage();
  const aliceStamp = await assertHmrBeacon(alice, aliceUrl, aliceRoot, 'alice', 'alice');
  const bobStamp = await assertHmrBeacon(bob, bobUrl, bobRoot, 'bob', 'bob');
  const aliceText = await alice.locator('[data-hmr-beacon]').innerText();
  const bobText = await bob.locator('[data-hmr-beacon]').innerText();
  const aU = aliceText.toUpperCase();
  const bU = bobText.toUpperCase();
  if (aU.includes(bobStamp.toUpperCase()) || bU.includes(aliceStamp.toUpperCase())) {
    throw new Error(
      `workspace isolation broken: alice=${JSON.stringify(aliceText)} bob=${JSON.stringify(bobText)}`
    );
  }
  console.log('dual-worktree-hmr: OK (hmr-no-nav + isolation)');
} finally {
  await browser.close();
}
