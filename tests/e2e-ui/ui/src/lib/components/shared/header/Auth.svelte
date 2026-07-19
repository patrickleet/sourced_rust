<script lang="ts">
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import type { Snippet } from 'svelte';

	const {
		children,
		onToggle
	}: {
		children?: Snippet;
		onToggle: () => void;
	} = $props();

	const isAuthenticated = $derived(!!page.data.session?.user);
	const user = $derived(page.data.session?.user);
	// /login starts Auth.js OIDC then shows our password form (Login V2 custom base URI).
	const signInHref = $derived(
		`/login?callbackUrl=${encodeURIComponent(page.url.pathname + (browser ? page.url.search : ''))}`
	);

	const toggleMenu = () => {
		onToggle();
	};

	const getInitials = (name: string | null | undefined, email: string | null | undefined): string => {
		if (name) {
			return name
				.split(' ')
				.map((n) => n[0])
				.join('')
				.toUpperCase()
				.slice(0, 2);
		}
		return email ? email.charAt(0).toUpperCase() : 'U';
	};
</script>

{#if isAuthenticated}
	<button type="button" onclick={toggleMenu} class="auth-avatar" aria-label="User menu">
		{#if user?.image}
			<img src={user.image} alt="Avatar" class="auth-avatar-img" />
		{:else}
			<span class="auth-avatar-initials">
				{getInitials(user?.name, user?.email)}
			</span>
		{/if}
	</button>
{:else}
	<a href={signInHref} class="cta-button">Sign in</a>
{/if}

{#if children}
	{@render children()}
{/if}
