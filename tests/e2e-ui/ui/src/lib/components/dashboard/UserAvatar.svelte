<script lang="ts">
	interface Props {
		/** User's name for initials */
		name?: string | null;
		/** User's email as fallback */
		email?: string | null;
		/** Avatar image URL */
		image?: string | null;
		/** Size variant */
		size?: 'sm' | 'md' | 'lg';
	}

	let { name, email, image, size = 'md' }: Props = $props();

	const initials = $derived(
		name
			? name.split(' ').map(n => n[0]).join('').toUpperCase().slice(0, 2)
			: email
				? email[0].toUpperCase()
				: null
	);
</script>

<div class="avatar size-{size}">
	{#if image}
		<img src={image} alt={name || 'User avatar'} />
	{:else if initials}
		<span class="initials">{initials}</span>
	{:else}
		<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
			<path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path>
			<circle cx="12" cy="7" r="4"></circle>
		</svg>
	{/if}
</div>

<style>
	.avatar {
		display: flex;
		align-items: center;
		justify-content: center;
		background: linear-gradient(135deg, var(--hops-navy) 0%, var(--hops-navy-light) 100%);
		border-radius: 12px;
		flex-shrink: 0;
		overflow: hidden;
		img {
			width: 100%;
			height: 100%;
			object-fit: cover;
		}
	}

	.size-sm {
		width: 40px;
		height: 40px;
		border-radius: 10px;
		.initials {
			font-size: 0.875rem; 
		}
		.icon {
			width: 20px; height: 20px; 
		}
	}

	.size-md {
		width: 56px;
		height: 56px;
		.initials {
			font-size: 1.125rem; 
		}
		.icon {
			width: 28px; height: 28px; 
		}
	}

	.size-lg {
		width: 72px;
		height: 72px;
		border-radius: 16px;
		.initials {
			font-size: 1.5rem; 
		}
		.icon {
			width: 36px; height: 36px; 
		}
	}

	.initials {
		font-family: var(--font-display);
		font-weight: 700;
		color: var(--hops-text-inverse);
		letter-spacing: 0.02em;
	}

	.icon {
		color: rgba(255, 255, 255, 0.7);
	}
</style>
