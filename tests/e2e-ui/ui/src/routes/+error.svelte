<script lang="ts">
	import { browser } from '$app/environment';
	import { page } from '$app/state';
	import { Button } from '$lib/components/shared/ui';

	let mounted = $state(false);
	let glitchActive = $state(false);
	let terminalLines = $state<string[]>([]);

	const errorMessages = [
		'kubectl get page --namespace=internet',
		'Error: resource "page" not found',
		'Checking alternate dimensions...',
		'Querying the multiverse...',
		'404: The requested resource has yeeted itself into the void',
		'',
		'SUGGESTION: Did you mean /home?'
	];

	// Svelte 5: prefer $effect over onMount for client-only setup.
	$effect(() => {
		if (!browser) return;

		mounted = true;
		let lineIndex = 0;
		let typeTimer: ReturnType<typeof setTimeout> | undefined;
		const typeNextLine = () => {
			if (lineIndex < errorMessages.length) {
				terminalLines = [...terminalLines, errorMessages[lineIndex]!];
				lineIndex++;
				typeTimer = setTimeout(typeNextLine, lineIndex === 1 ? 800 : 400);
			}
		};
		const startTimer = setTimeout(typeNextLine, 600);

		const glitchInterval = setInterval(() => {
			glitchActive = true;
			setTimeout(() => (glitchActive = false), 150);
		}, 3000 + Math.random() * 2000);

		return () => {
			clearTimeout(startTimer);
			if (typeTimer) clearTimeout(typeTimer);
			clearInterval(glitchInterval);
		};
	});

	const status = $derived(page.status);
	const message = $derived(page.error?.message || 'Page not found');
</script>

<svelte:head>
	<title>{status} | Hops</title>
</svelte:head>

<div class="error-page" class:mounted class:glitch={glitchActive}>
	<!-- Scan lines overlay -->
	<div class="scanlines"></div>

	<!-- Floating particles -->
	<div class="particles">
		{#each Array(12) as _, i (i)}
			<div
				class="particle"
				style="--delay: {i * 0.3}s; --x: {Math.random() * 100}%; --duration: {8 + Math.random() * 6}s"
			></div>
		{/each}
	</div>

	<div class="error-content">
		<!-- ASCII Art 404 -->
		<div class="ascii-block">
			<pre class="ascii-art" aria-hidden="true">{`
    ██╗  ██╗ ██████╗ ██╗  ██╗
    ██║  ██║██╔═████╗██║  ██║
    ███████║██║██╔██║███████║
    ╚════██║████╔╝██║╚════██║
         ██║╚██████╔╝     ██║
         ╚═╝ ╚═════╝      ╚═╝
`}</pre>
			<div class="glitch-layers" aria-hidden="true">
				<pre class="ascii-art glitch-1">{`
    ██╗  ██╗ ██████╗ ██╗  ██╗
    ██║  ██║██╔═████╗██║  ██║
    ███████║██║██╔██║███████║
    ╚════██║████╔╝██║╚════██║
         ██║╚██████╔╝     ██║
         ╚═╝ ╚═════╝      ╚═╝
`}</pre>
				<pre class="ascii-art glitch-2">{`
    ██╗  ██╗ ██████╗ ██╗  ██╗
    ██║  ██║██╔═████╗██║  ██║
    ███████║██║██╔██║███████║
    ╚════██║████╔╝██║╚════██║
         ██║╚██████╔╝     ██║
         ╚═╝ ╚═════╝      ╚═╝
`}</pre>
			</div>
		</div>

		<!-- Hop character -->
		<div class="hop-character">
			<div class="hop-body">
				<div class="hop-leaf hop-leaf-1"></div>
				<div class="hop-leaf hop-leaf-2"></div>
				<div class="hop-leaf hop-leaf-3"></div>
				<div class="hop-cone"></div>
				<div class="hop-face">
					<div class="hop-eye hop-eye-left"></div>
					<div class="hop-eye hop-eye-right"></div>
					<div class="hop-mouth"></div>
				</div>
			</div>
			<div class="hop-shadow"></div>
		</div>

		<!-- Terminal window -->
		<div class="terminal">
			<div class="terminal-header">
				<div class="terminal-dots">
					<span class="dot dot-red"></span>
					<span class="dot dot-yellow"></span>
					<span class="dot dot-green"></span>
				</div>
				<span class="terminal-title">hops-cluster ~ lost-pod</span>
			</div>
			<div class="terminal-body">
				{#each terminalLines as line, i (i)}
					<div class="terminal-line" style="--line-delay: {i * 0.1}s">
						{#if i === 0}
							<span class="prompt">$</span>
						{/if}
						<span class:error-text={line.includes('Error') || line.includes('404')} class:suggestion={line.includes('SUGGESTION')}>
							{line}
						</span>
						{#if i === terminalLines.length - 1 && terminalLines.length < errorMessages.length}
							<span class="cursor"></span>
						{/if}
					</div>
				{/each}
				{#if terminalLines.length === 0}
					<div class="terminal-line">
						<span class="prompt">$</span>
						<span class="cursor"></span>
					</div>
				{/if}
			</div>
		</div>

		<!-- Message and CTA -->
		<div class="error-message">
			<h1 class="error-title">
				Lost in the Cluster
			</h1>
			<p class="error-subtitle">
				This page doesn't exist in any namespace we control.
				<br />
				Maybe it was never deployed, or perhaps it drifted into the void.
			</p>
		</div>

		<div class="error-actions">
			<Button href="/" variant="primary" size="lg">
				<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
					<path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>
					<polyline points="9 22 9 12 15 12 15 22"/>
				</svg>
				Back to Home
			</Button>
			<Button href="/launch" variant="outlined" size="lg">
				Launch a Platform Instead
			</Button>
		</div>

		<!-- Fun stats -->
		<div class="fun-stats">
			<div class="stat">
				<span class="stat-value">0</span>
				<span class="stat-label">pods found</span>
			</div>
			<div class="stat">
				<span class="stat-value">{status}</span>
				<span class="stat-label">status code</span>
			</div>
			<div class="stat">
				<span class="stat-value">100%</span>
				<span class="stat-label">confusion</span>
			</div>
		</div>
	</div>
</div>

<style>
/* Base layout */
.error-page {
	min-height: 100vh;
	display: flex;
	align-items: center;
	justify-content: center;
	background: var(--hops-navy-deep);
	position: relative;
	overflow: hidden;
	padding: 2rem;
	padding-top: 7rem; /* Account for navbar (5rem) + breathing room */
}

/* Scanline effect */
.scanlines {
	position: absolute;
	inset: 0;
	background: repeating-linear-gradient(
		0deg,
		transparent,
		transparent 2px,
		rgba(0, 0, 0, 0.1) 2px,
		rgba(0, 0, 0, 0.1) 4px
	);
	pointer-events: none;
	z-index: 100;
}

/* Floating particles */
.particles {
	position: absolute;
	inset: 0;
	overflow: hidden;
	pointer-events: none;
}

.particle {
	position: absolute;
	width: 4px;
	height: 4px;
	background: var(--hops-orange);
	border-radius: 50%;
	left: var(--x);
	bottom: -20px;
	opacity: 0.3;
	animation: floatUp var(--duration) ease-in-out infinite;
	animation-delay: var(--delay);
}

@keyframes floatUp {
	0% {
		transform: translateY(0) rotate(0deg);
		opacity: 0;
	}
	10% {
		opacity: 0.4;
	}
	90% {
		opacity: 0.4;
	}
	100% {
		transform: translateY(-100vh) rotate(360deg);
		opacity: 0;
	}
}

/* Content container */
.error-content {
	position: relative;
	z-index: 10;
	text-align: center;
	max-width: 700px;
	width: 100%;
	opacity: 0;
	transform: translateY(30px);
	transition: all 0.8s var(--ease-out-expo);
}

.mounted .error-content {
	opacity: 1;
	transform: translateY(0);
}

/* ASCII Art Block */
.ascii-block {
	position: relative;
	margin-bottom: 2rem;
}

.ascii-art {
	font-family: var(--font-mono);
	font-size: clamp(0.4rem, 2vw, 0.7rem);
	line-height: 1.1;
	color: var(--hops-orange);
	margin: 0;
	white-space: pre;
	text-shadow: 0 0 20px rgba(230, 154, 45, 0.5);
}

.glitch-layers {
	position: absolute;
	inset: 0;
	pointer-events: none;
}

.glitch-1,
.glitch-2 {
	position: absolute;
	top: 0;
	left: 0;
	opacity: 0;
}

.glitch .glitch-1 {
	opacity: 0.8;
	transform: translateX(-3px);
	color: #ff6b6b;
	animation: glitchShift1 0.15s steps(2) infinite;
}

.glitch .glitch-2 {
	opacity: 0.8;
	transform: translateX(3px);
	color: #4ecdc4;
	animation: glitchShift2 0.15s steps(2) infinite;
}

@keyframes glitchShift1 {
	0%, 100% { clip-path: inset(40% 0 20% 0); transform: translateX(-3px); }
	50% { clip-path: inset(10% 0 60% 0); transform: translateX(3px); }
}

@keyframes glitchShift2 {
	0%, 100% { clip-path: inset(20% 0 40% 0); transform: translateX(3px); }
	50% { clip-path: inset(60% 0 10% 0); transform: translateX(-3px); }
}

/* Hop Character */
.hop-character {
	position: relative;
	width: 80px;
	height: 100px;
	margin: 0 auto 2rem;
	animation: hopBounce 2s ease-in-out infinite;
}

@keyframes hopBounce {
	0%, 100% { transform: translateY(0); }
	50% { transform: translateY(-15px); }
}

.hop-body {
	position: relative;
	width: 60px;
	height: 80px;
	margin: 0 auto;
}

.hop-cone {
	position: absolute;
	bottom: 0;
	left: 50%;
	transform: translateX(-50%);
	width: 50px;
	height: 65px;
	background: linear-gradient(135deg, #7cb342 0%, #558b2f 50%, #33691e 100%);
	border-radius: 50% 50% 45% 45% / 60% 60% 40% 40%;
	box-shadow: inset -8px -8px 20px rgba(0, 0, 0, 0.2),
	            inset 5px 5px 15px rgba(255, 255, 255, 0.1);
}

.hop-cone::before {
	content: '';
	position: absolute;
	top: 10%;
	left: 15%;
	right: 15%;
	bottom: 20%;
	background: repeating-linear-gradient(
		-30deg,
		transparent,
		transparent 3px,
		rgba(255, 255, 255, 0.1) 3px,
		rgba(255, 255, 255, 0.1) 6px
	);
	border-radius: inherit;
}

.hop-leaf {
	position: absolute;
	width: 20px;
	height: 30px;
	background: linear-gradient(135deg, #8bc34a 0%, #689f38 100%);
	border-radius: 50% 50% 50% 50% / 60% 60% 40% 40%;
}

.hop-leaf-1 {
	top: -5px;
	left: 50%;
	transform: translateX(-50%) rotate(-10deg);
	animation: leafWave1 3s ease-in-out infinite;
}

.hop-leaf-2 {
	top: 5px;
	left: -5px;
	transform: rotate(-40deg);
	animation: leafWave2 3s ease-in-out infinite 0.5s;
}

.hop-leaf-3 {
	top: 5px;
	right: -5px;
	transform: rotate(40deg);
	animation: leafWave3 3s ease-in-out infinite 1s;
}

@keyframes leafWave1 {
	0%, 100% { transform: translateX(-50%) rotate(-10deg); }
	50% { transform: translateX(-50%) rotate(10deg); }
}

@keyframes leafWave2 {
	0%, 100% { transform: rotate(-40deg); }
	50% { transform: rotate(-30deg); }
}

@keyframes leafWave3 {
	0%, 100% { transform: rotate(40deg); }
	50% { transform: rotate(30deg); }
}

.hop-face {
	position: absolute;
	top: 35%;
	left: 50%;
	transform: translateX(-50%);
	width: 35px;
	height: 25px;
}

.hop-eye {
	position: absolute;
	top: 0;
	width: 8px;
	height: 10px;
	background: #1a2744;
	border-radius: 50%;
	animation: blink 4s ease-in-out infinite;
}

.hop-eye-left {
	left: 5px;
}

.hop-eye-right {
	right: 5px;
}

.hop-eye::after {
	content: '';
	position: absolute;
	top: 2px;
	right: 1px;
	width: 3px;
	height: 3px;
	background: white;
	border-radius: 50%;
}

@keyframes blink {
	0%, 45%, 55%, 100% { transform: scaleY(1); }
	50% { transform: scaleY(0.1); }
}

.hop-mouth {
	position: absolute;
	bottom: 0;
	left: 50%;
	transform: translateX(-50%);
	width: 12px;
	height: 6px;
	border: 2px solid #1a2744;
	border-top: none;
	border-radius: 0 0 12px 12px;
}

.hop-shadow {
	position: absolute;
	bottom: -10px;
	left: 50%;
	transform: translateX(-50%);
	width: 40px;
	height: 10px;
	background: rgba(0, 0, 0, 0.3);
	border-radius: 50%;
	animation: shadowPulse 2s ease-in-out infinite;
}

@keyframes shadowPulse {
	0%, 100% { transform: translateX(-50%) scale(1); opacity: 0.3; }
	50% { transform: translateX(-50%) scale(0.7); opacity: 0.5; }
}

/* Terminal */
.terminal {
	background: rgba(15, 24, 41, 0.9);
	border: 1px solid rgba(230, 154, 45, 0.3);
	border-radius: 12px;
	overflow: hidden;
	text-align: left;
	margin-bottom: 2.5rem;
	box-shadow: 0 20px 60px rgba(0, 0, 0, 0.4),
	            0 0 40px rgba(230, 154, 45, 0.1);
}

.terminal-header {
	display: flex;
	align-items: center;
	gap: 1rem;
	padding: 0.75rem 1rem;
	background: rgba(26, 39, 68, 0.8);
	border-bottom: 1px solid rgba(255, 255, 255, 0.1);
}

.terminal-dots {
	display: flex;
	gap: 6px;
}

.dot {
	width: 12px;
	height: 12px;
	border-radius: 50%;
}

.dot-red { background: #ff5f56; }
.dot-yellow { background: #ffbd2e; }
.dot-green { background: #27ca40; }

.terminal-title {
	font-family: var(--font-mono);
	font-size: 0.75rem;
	color: rgba(255, 255, 255, 0.5);
}

.terminal-body {
	padding: 1.25rem;
	min-height: 160px;
}

.terminal-line {
	font-family: var(--font-mono);
	font-size: 0.85rem;
	color: rgba(255, 255, 255, 0.8);
	line-height: 1.8;
	animation: lineAppear 0.3s ease-out both;
	animation-delay: var(--line-delay);
}

@keyframes lineAppear {
	from {
		opacity: 0;
		transform: translateY(5px);
	}
	to {
		opacity: 1;
		transform: translateY(0);
	}
}

.prompt {
	color: var(--hops-orange);
	margin-right: 0.5rem;
}

.error-text {
	color: #ff6b6b;
}

.suggestion {
	color: #4ecdc4;
}

.cursor {
	display: inline-block;
	width: 8px;
	height: 16px;
	background: var(--hops-orange);
	margin-left: 2px;
	animation: cursorBlink 1s steps(1) infinite;
	vertical-align: text-bottom;
}

@keyframes cursorBlink {
	0%, 50% { opacity: 1; }
	51%, 100% { opacity: 0; }
}

/* Message */
.error-message {
	margin-bottom: 2rem;
}

.error-title {
	font-family: var(--font-display);
	font-size: clamp(1.75rem, 5vw, 2.5rem);
	font-weight: 800;
	color: var(--hops-text-inverse);
	margin-bottom: 1rem;
	letter-spacing: -0.02em;
}

.error-subtitle {
	font-size: 1rem;
	color: rgba(255, 255, 255, 0.6);
	line-height: 1.7;
	margin: 0;
}

/* Actions */
.error-actions {
	display: flex;
	gap: 1rem;
	justify-content: center;
	flex-wrap: wrap;
	margin-bottom: 3rem;
}

:global(.error-actions .button) {
	display: inline-flex;
	align-items: center;
	gap: 0.5rem;
}

/* Fun stats */
.fun-stats {
	display: flex;
	justify-content: center;
	gap: 3rem;
	padding-top: 2rem;
	border-top: 1px solid rgba(255, 255, 255, 0.1);
}

.stat {
	text-align: center;
}

.stat-value {
	display: block;
	font-family: var(--font-mono);
	font-size: 1.5rem;
	font-weight: 700;
	color: var(--hops-orange);
}

.stat-label {
	font-size: 0.75rem;
	color: rgba(255, 255, 255, 0.4);
	text-transform: uppercase;
	letter-spacing: 0.1em;
}

/* Mobile adjustments */
@media (--tablet) {
	.ascii-art {
		font-size: 0.35rem;
	}

	.hop-character {
		transform: scale(0.8);
		margin-bottom: 1rem;
	}

	.terminal-body {
		padding: 1rem;
		min-height: 140px;
	}

	.terminal-line {
		font-size: 0.75rem;
	}

	.fun-stats {
		gap: 1.5rem;
	}
}

@media (--mobile) {
	.ascii-art {
		font-size: 0.28rem;
	}

	.fun-stats {
		flex-direction: column;
		gap: 1rem;
	}
}
</style>
