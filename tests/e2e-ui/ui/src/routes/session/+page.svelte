<script lang="ts">
	import { browser } from '$app/environment';
	import { BadgeCheck, LockKeyhole, LogOut, ShieldCheck, UserRound } from '@lucide/svelte';
	import { Button, SimplePage } from '$lib/components/shared/ui';
	import TokenInspector from './TokenInspector.svelte';
	import type { PageData } from './$types';

	interface SessionUser {
		id?: string | null;
		name?: string | null;
		email?: string | null;
		image?: string | null;
		username?: string | null;
		emailVerified?: boolean | null;
		groups?: string[] | null;
	}

	interface SessionLike {
		user?: SessionUser | null;
		accessToken?: string | null;
		idToken?: string | null;
		expires?: string | null;
		expiresAt?: number | null;
		hasAccessToken?: boolean | null;
		hasRefreshToken?: boolean | null;
		hasIdToken?: boolean | null;
		error?: string | null;
	}

	let { data }: { data: PageData } = $props();

	let now = $state(currentUnixSeconds());
	const session = $derived(data.session as SessionLike | null | undefined);
	const user = $derived(session?.user);
	const expiresAt = $derived(resolveExpiresAt(session));
	const expiresAtLabel = $derived(formatExpiresAt(expiresAt, session?.expires));
	const expiresIn = $derived(formatCountdown(expiresAt, now));
	const isExpired = $derived(expiresAt !== undefined && expiresAt <= now);
	const isAdmin = $derived(Boolean(user?.groups?.includes('admins')));

	$effect(() => {
		if (!browser) return;

		const timer = window.setInterval(() => {
			now = currentUnixSeconds();
		}, 1_000);

		return () => window.clearInterval(timer);
	});

	function currentUnixSeconds() {
		return Math.floor(Date.now() / 1_000);
	}

	function displayName(value: SessionUser | null | undefined) {
		return value?.name || value?.email || value?.username || 'User';
	}

	function initials(value: SessionUser | null | undefined) {
		const nameInitials = value?.name
			?.split(/\s+/)
			.filter(Boolean)
			.map((part) => part[0])
			.join('')
			.slice(0, 2);

		if (nameInitials) return nameInitials.toUpperCase();

		const fallback = value?.email || value?.username || 'U';
		return fallback[0]?.toUpperCase() ?? 'U';
	}

	function resolveExpiresAt(value: SessionLike | null | undefined) {
		if (typeof value?.expiresAt === 'number' && Number.isFinite(value.expiresAt)) {
			return value.expiresAt;
		}

		const expires = value?.expires ? Date.parse(value.expires) : Number.NaN;
		if (Number.isFinite(expires)) return Math.floor(expires / 1_000);

		return undefined;
	}

	function formatExpiresAt(expiresAt: number | undefined, expires: string | null | undefined) {
		const date = expiresAt ? new Date(expiresAt * 1_000) : expires ? new Date(expires) : undefined;
		if (!date || Number.isNaN(date.getTime())) return 'Unavailable';

		return date.toLocaleString(undefined, {
			year: 'numeric',
			month: 'short',
			day: 'numeric',
			hour: 'numeric',
			minute: '2-digit'
		});
	}

	function formatCountdown(expiresAt: number | undefined, current: number) {
		if (!expiresAt) return 'Unavailable';
		if (expiresAt <= current) return 'Expired';

		const remaining = expiresAt - current;
		const days = Math.floor(remaining / 86_400);
		const hours = Math.floor((remaining % 86_400) / 3_600);
		const minutes = Math.floor((remaining % 3_600) / 60);
		const seconds = remaining % 60;

		if (days > 0) return `${days}d ${hours.toString().padStart(2, '0')}h`;
		if (hours > 0) return `${hours}h ${minutes.toString().padStart(2, '0')}m`;
		if (minutes > 0) return `${minutes}m ${seconds.toString().padStart(2, '0')}s`;
		return `${seconds}s`;
	}
</script>

<SimplePage title="Session" maxWidth="lg">
	{#if user}
		<div class="session-view">
			<section class="session-profile" aria-label="Signed-in account">
				<div class="session-avatar" aria-hidden="true">{initials(user)}</div>
				<div class="session-identity">
					<h1>Session</h1>
					<h2>{displayName(user)}</h2>
					{#if user.email}
						<p class="muted">{user.email}</p>
					{/if}
					{#if user.groups?.length}
						<div class="pill-row" aria-label="Groups">
							{#each user.groups as group (group)}
								<span class="group-pill">{group}</span>
							{/each}
						</div>
					{/if}
				</div>
			</section>

			{#if session?.error}
				<p class="session-alert">Token refresh reported: {session.error}</p>
			{/if}

			<section class="session-grid" aria-label="Session details">
				<div class="session-row">
					<span>User ID</span>
					<strong class="mono">{user.id || 'Unavailable'}</strong>
				</div>
				{#if user.username}
					<div class="session-row">
						<span>Username</span>
						<strong>{user.username}</strong>
					</div>
				{/if}
				{#if typeof user.emailVerified === 'boolean'}
					<div class="session-row">
						<span>Email Verified</span>
						<strong class:verified={user.emailVerified}>
							{#if user.emailVerified}
								<BadgeCheck size={17} strokeWidth={2.2} aria-hidden="true" />
							{/if}
							{user.emailVerified ? 'Verified' : 'Unverified'}
						</strong>
					</div>
				{/if}
				<div class="session-row">
					<span>Expires At</span>
					<strong>{expiresAtLabel}</strong>
				</div>
				<div class="session-row">
					<span>Expires In</span>
					<strong class="session-countdown" class:expired={isExpired}>{expiresIn}</strong>
				</div>
			</section>

			<section class="token-inspector-list" aria-label="Session tokens">
				<TokenInspector
					label="Access Token"
					token={session?.accessToken}
					present={Boolean(session?.hasAccessToken)}
					statusLabel="Available to app"
				/>
				<TokenInspector
					label="ID Token"
					token={session?.idToken}
					present={Boolean(session?.hasIdToken)}
					statusLabel="Available to app"
				/>
				<TokenInspector
					label="Refresh Token"
					present={Boolean(session?.hasRefreshToken)}
					protectedMessage="Stored in an HttpOnly cookie and unavailable to page scripts."
				/>
			</section>

			<div class="button-row center">
				<Button variant="secondary" href="/protected">
					<ShieldCheck size={18} strokeWidth={2.2} aria-hidden="true" />
					Protected Page
				</Button>
				{#if isAdmin}
					<Button variant="secondary" href="/admin">
						<UserRound size={18} strokeWidth={2.2} aria-hidden="true" />
						Admin Panel
					</Button>
				{/if}
				<Button variant="primary" href="/signout">
					<LogOut size={18} strokeWidth={2.2} aria-hidden="true" />
					Sign Out
				</Button>
			</div>
		</div>
	{:else}
		<div class="empty-state">
			<div class="empty-icon" aria-hidden="true">
				<LockKeyhole size={42} strokeWidth={1.7} />
			</div>
			<h1 class="empty-title">Not signed in</h1>
			<p class="empty-text">Sign in to view your session information and account details.</p>
			<Button variant="primary" href="/signin?callbackUrl=/session">Sign In</Button>
		</div>
	{/if}
</SimplePage>

<style>
	.session-view {
		width: 100%;
		text-align: left;

	}

	.session-profile {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr);
		align-items: center;
		gap: 1.15rem;
		padding: 1.35rem 1.4rem;
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: var(--df-radius-lg, 10px);

	}

	.session-avatar {
		width: 68px;
		height: 68px;
		border-radius: 12px;
		display: grid;
		place-items: center;
		background: var(--wf-ink, #1c1c1a);
		color: var(--wf-bg, #f6f5f2);
		font-family: var(--wf-sans, system-ui, sans-serif);
		font-size: 1.2rem;
		font-weight: 600;
		letter-spacing: 0.02em;

	}

	.session-identity {
		min-width: 0;

		h1 {
			margin: 0 0 0.2rem;
			font-family: var(--wf-serif, Georgia, serif);
			font-size: clamp(1.5rem, 3vw, 1.85rem);
			font-weight: 500;
			letter-spacing: -0.02em;
			line-height: 1.1;
			color: var(--wf-ink, #1c1c1a);

		}
		h2 {
			margin: 0;
			color: var(--wf-ink, #1c1c1a);
			font-family: var(--wf-sans, system-ui, sans-serif);
			font-size: 1.15rem;
			font-weight: 600;
			line-height: 1.25;
			overflow-wrap: anywhere;

		}
	}

	.muted {
		margin: 0.3rem 0 0;
		color: var(--wf-ink-muted, #8a8a82);
		font-size: 0.95rem;
		overflow-wrap: anywhere;

	}

	.pill-row {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-top: 0.75rem;

	}

	.group-pill {
		display: inline-flex;
		align-items: center;
		max-width: 100%;
		min-height: 26px;
		padding: 0.2rem 0.55rem;
		border-radius: 999px;
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		border: 1px solid var(--wf-line, #e2e0d9);
		color: var(--wf-accent, #3d5a80);
		font-size: 0.75rem;
		font-weight: 600;
		overflow-wrap: anywhere;

	}

	.session-alert {
		margin: 1rem 0 0;
		padding: 0.75rem 0.95rem;
		border-radius: var(--wf-radius, 6px);
		border: 1px solid var(--wf-line, #e2e0d9);
		background: var(--wf-accent-soft, rgba(61, 90, 128, 0.08));
		color: var(--wf-ink, #1c1c1a);
		font-size: 0.92rem;
		font-weight: 500;

	}

	.session-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 0.65rem;
		margin-top: 1rem;

		@media (--tablet) {
			grid-template-columns: 1fr;

		}
	}

	.session-row {
		display: grid;
		grid-template-columns: minmax(7.5rem, 0.4fr) minmax(0, 1fr);
		align-items: center;
		gap: 0.85rem;
		min-height: 56px;
		padding: 0.75rem 0.95rem;
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: var(--wf-radius, 6px);

		span {
			color: var(--wf-ink-muted, #8a8a82);
			font-size: 0.72rem;
			font-weight: 600;
			text-transform: uppercase;
			letter-spacing: 0.05em;

		}
		strong {
			display: inline-flex;
			align-items: center;
			gap: 0.35rem;
			min-width: 0;
			color: var(--wf-ink, #1c1c1a);
			font-size: 0.95rem;
			font-weight: 600;
			line-height: 1.35;
			overflow-wrap: anywhere;

		}
		.mono {
			font-family: var(--wf-mono, ui-monospace, monospace);
			font-size: 0.85rem;
			font-weight: 500;

		}
	}

	.session-row .verified,
		.session-countdown {
		color: var(--wf-success, #2f6f4e);

	}

	.session-countdown.expired {
		color: var(--wf-danger, #b33a3a);

	}

	.token-inspector-list {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
		margin-top: 1rem;

	}

	.button-row {
		display: flex;
		flex-wrap: wrap;
		gap: 0.65rem;
		margin-top: 1.25rem;

		&.center {
			justify-content: center;

		}
	}

	.empty-state {
		text-align: center;
		padding: 3.5rem 1.5rem;

	}

	.empty-icon {
		width: 72px;
		height: 72px;
		margin: 0 auto 1.25rem;
		background: var(--wf-bg-elevated, #fff);
		border: 1px solid var(--wf-line, #e2e0d9);
		border-radius: 14px;
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--wf-ink, #1c1c1a);

	}

	.empty-title {
		margin: 0 0 0.45rem;
		color: var(--wf-ink, #1c1c1a);
		font-family: var(--wf-serif, Georgia, serif);
		font-size: clamp(1.5rem, 4vw, 2rem);
		font-weight: 500;
		letter-spacing: -0.02em;

	}

	.empty-text {
		max-width: 320px;
		margin: 0 auto 1.75rem;
		color: var(--wf-ink-soft, #5c5c56);
		font-size: 0.98rem;

	}

	@media (--mobile) {
		.session-profile {
			grid-template-columns: 1fr;
			justify-items: center;
			text-align: center;

		}
		.session-identity h2 {
			font-size: 1.05rem;

		}
		.pill-row {
			justify-content: center;

		}
		.session-row {
			grid-template-columns: 1fr;
			gap: 0.3rem;

		}
		.button-row {
			align-items: stretch;

		}
	}
</style>
