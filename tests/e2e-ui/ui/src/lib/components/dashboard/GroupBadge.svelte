<script lang="ts">
	interface Props {
		/** Group name */
		name: string;
	}

	let { name }: Props = $props();
</script>

<span class="group-badge">{name}</span>

<style>
.group-badge {
	display: inline-block;
	padding: 0.25rem 0.6rem;
	background: rgba(26, 39, 68, 0.08);
	border-radius: 5px;
	font-size: 0.7rem;
	font-weight: 600;
	color: var(--hops-navy);
	text-transform: lowercase;
}
</style>
