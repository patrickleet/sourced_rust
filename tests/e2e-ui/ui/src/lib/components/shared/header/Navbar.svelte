<script lang="ts">
	import { page } from '$app/state';
	import Auth from '$lib/components/shared/header/Auth.svelte';
	import AccountMenu from '$lib/components/shared/menus/AccountMenu.svelte';
	import { engineRoleFromGroups, isAdminEngineRole } from '$lib/roles';

	let isMenuOpen = $state(false);
	let scrolled = $state(false);
	let accountMenuOpen = $state(false);

	const isAuthenticated = $derived(!!page.data.session?.user);
	/** Prefer layout `engineRole`; fall back to session groups (Zitadel project roles). */
	const isAdmin = $derived(
		isAdminEngineRole(page.data.engineRole) ||
			isAdminEngineRole(
				engineRoleFromGroups(
					(page.data.session?.user as { groups?: string[] } | undefined)?.groups
				)
			)
	);
	const currentPath = $derived(page.url.pathname);

	const isActive = (path: string) => {
		if (path === '/') return currentPath === '/';
		return currentPath.startsWith(path);
	};

	const toggleAccountMenu = () => {
		accountMenuOpen = !accountMenuOpen;
	};

	const toggleMenu = () => {
		isMenuOpen = !isMenuOpen;
	};

	$effect(() => {
		const handleScroll = () => {
			scrolled = window.scrollY > 50;
		};
		if (typeof window !== 'undefined') {
			window.addEventListener('scroll', handleScroll);
			return () => window.removeEventListener('scroll', handleScroll);
		}
	});
</script>

<nav class="navbar" class:scrolled aria-label="main navigation">
	<div class="navbar-container">
		<div class="navbar-brand">
			<a href="/" class="brand-link">
				<span class="brand-mark" aria-hidden="true">df</span>
				e2e-ui
			</a>
		</div>

		<button
			class="navbar-burger"
			class:is-active={isMenuOpen}
			aria-label="menu"
			aria-expanded={isMenuOpen}
			onclick={toggleMenu}
		>
			<span></span>
			<span></span>
			<span></span>
		</button>

		<div class="navbar-menu" class:is-active={isMenuOpen}>
			<div class="navbar-links">
				<a href="/" class="nav-link" class:active={isActive('/')} onclick={() => (isMenuOpen = false)}
					>Home</a
				>
				<!-- Lobby is readable anonymously (e2e-ui-public). -->
				<a
					href="/chat"
					class="nav-link"
					class:active={isActive('/chat')}
					onclick={() => (isMenuOpen = false)}>Chat</a
				>
				{#if isAuthenticated}
					<a
						href="/todos"
						class="nav-link"
						class:active={isActive('/todos')}
						onclick={() => (isMenuOpen = false)}>Todos</a
					>
					<a
						href="/blob"
						class="nav-link"
						class:active={isActive('/blob')}
						onclick={() => (isMenuOpen = false)}>Blob</a
					>
					<a
						href="/session"
						class="nav-link"
						class:active={isActive('/session')}
						onclick={() => (isMenuOpen = false)}>Session</a
					>
					{#if isAdmin}
						<a
							href="/admin"
							class="nav-link nav-link-admin"
							class:active={isActive('/admin')}
							onclick={() => (isMenuOpen = false)}>Admin</a
						>
					{/if}
				{/if}
			</div>

			<div class="navbar-cta">
				{#if !isAuthenticated}
					<a
						href="/signup?callbackUrl=/todos"
						class="cta-button-outline"
						onclick={() => (isMenuOpen = false)}
					>
						Create account
					</a>
				{/if}
				<Auth onToggle={toggleAccountMenu} />
			</div>
		</div>
	</div>
</nav>

{#if accountMenuOpen}
	<AccountMenu bind:accountMenuOpen />
{/if}
