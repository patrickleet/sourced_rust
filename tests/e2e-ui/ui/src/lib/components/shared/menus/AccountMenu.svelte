<script lang="ts">
  import { page } from '$app/state';

  let { accountMenuOpen = $bindable(false) } = $props();

  const user = $derived(page.data.session?.user);
  const displayName = $derived(user?.name || user?.email || user?.username || 'Signed in');
</script>

<div class="account-menu-overlay">
    <div class="account-menu-modal">
      <h2 class="account-menu-title">Account Actions</h2>
      {#if user}
        <div class="account-menu-user">
          <strong>{displayName}</strong>
          {#if user.email}
            <span>{user.email}</span>
          {/if}
        </div>
      {/if}
      <a
        href="/todos"
        class="account-menu-btn account-menu-btn-primary"
        onclick={() => accountMenuOpen = false}
      >
        Todos
      </a>
      <a
        href="/chat"
        class="account-menu-btn account-menu-btn-secondary"
        onclick={() => accountMenuOpen = false}
      >
        Chat
      </a>
      <a
        href="/session"
        class="account-menu-btn account-menu-btn-secondary"
        onclick={() => accountMenuOpen = false}
      >
        Session Info
      </a>
      <a
        href="http://localhost:18080/ui/console/users/me"
        class="account-menu-btn account-menu-btn-secondary"
      >
        Manage Account
      </a>
      <a
        href="/signout"
        class="account-menu-btn account-menu-btn-danger"
      >
        Sign Out
      </a>
      <button
        onclick={() => accountMenuOpen = false}
        class="account-menu-btn account-menu-btn-neutral"
      >
        Close
      </button>
    </div>
  </div>

<style>
	/* Mobile-first */

	.account-menu-overlay {
		position: fixed;
		top: 5rem;
		right: 0.5rem;
		left: 0.5rem;
		z-index: 1100;
		animation: fadeIn 0.2s var(--ease-out-expo);

		@media (--mobile-up) {
			right: 1rem;
			left: auto;
		}
	}

	.account-menu-modal {
		background: var(--hops-bg-white);
		border: 1px solid var(--hops-border-strong);
		border-radius: 12px;
		box-shadow: var(--shadow-lg);
		padding: 1.25rem;
		min-width: auto;
		display: flex;
		flex-direction: column;
		gap: 0.5rem;

		@media (--mobile-up) {
			min-width: 200px;
		}
	}

	.account-menu-title {
		font-family: var(--font-display);
		font-size: 0.875rem;
		font-weight: 700;
		color: var(--hops-text-muted);
		text-transform: uppercase;
		letter-spacing: 0;
		margin: 0 0 0.5rem 0;
		padding-bottom: 0.5rem;
		border-bottom: 1px solid var(--hops-border);
	}

	.account-menu-user {
		display: flex;
		flex-direction: column;
		gap: 0.15rem;
		padding: 0.25rem 0 0.75rem;
		border-bottom: 1px solid var(--hops-border);
		margin-bottom: 0.25rem;
		strong {
			font-family: var(--font-display);
			font-size: 0.95rem;
			color: var(--hops-navy);
		}
		span {
			font-size: 0.82rem;
			color: var(--hops-text-muted);
			overflow-wrap: anywhere;
		}
	}

	.account-menu-btn {
		display: flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-body);
		font-size: 0.9rem;
		font-weight: 600;
		padding: 0.75rem 1rem;
		border-radius: 8px;
		border: none;
		cursor: pointer;
		text-decoration: none;
		transition: all 0.2s var(--ease-out-expo);
		text-align: center;

		&.account-menu-btn-primary {
			background: var(--hops-orange);
			color: var(--hops-text-inverse);

			&:hover {
				background: var(--hops-orange-light);
				transform: translateY(-1px);
			}
		}

		&.account-menu-btn-secondary {
			background: var(--hops-navy);
			color: var(--hops-text-inverse);

			&:hover {
				background: var(--hops-navy-light);
				transform: translateY(-1px);
			}
		}

		&.account-menu-btn-danger {
			background: var(--hops-danger);
			color: var(--hops-text-inverse);

			&:hover {
				background: #c53030;
				transform: translateY(-1px);
			}
		}

		&.account-menu-btn-neutral {
			background: transparent;
			color: var(--hops-text-secondary);
			border: 1px solid var(--hops-border-strong);

			&:hover {
				background: var(--hops-bg-light);
				color: var(--hops-text-primary);
			}
		}
	}
</style>
