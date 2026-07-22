<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { audio } from '../stores/audio.svelte';

  let canvas: HTMLCanvasElement | undefined = $state();
  let canvasWidth = $state(600);
  let canvasHeight = $state(80);
  let resizeObserver: ResizeObserver | null = null;

  function draw() {
    if (!canvas) return;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;

    const w = canvas.width;
    const h = canvas.height;
    const data = audio.state.waveformData;

    const style = getComputedStyle(canvas);
    ctx.fillStyle = style.getPropertyValue('--bg-tertiary').trim() || '#2c2e33';
    ctx.fillRect(0, 0, w, h);

    ctx.strokeStyle = style.getPropertyValue('--border').trim() || '#373a40';
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(0, h / 2);
    ctx.lineTo(w, h / 2);
    ctx.stroke();

    if (data.length === 0) return;

    const accent = style.getPropertyValue('--accent').trim() || '#5c7cfa';
    ctx.fillStyle = accent;

    const barCount = Math.min(data.length, 64);
    const startIdx = Math.max(0, data.length - barCount);
    const barW = w / barCount;
    const gap = Math.max(1, barW * 0.2);

    for (let i = 0; i < barCount; i++) {
      const sample = Math.abs(data[startIdx + i] ?? 0);
      const barH = Math.max(2, sample * (h * 0.9));
      const x = i * barW + gap / 2;
      const y = (h - barH) / 2;
      ctx.fillRect(x, y, barW - gap, barH);
    }
  }

  function updateCanvasSize() {
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    canvasWidth = Math.round(rect.width * dpr);
    canvasHeight = Math.round(rect.height * dpr);
    draw();
  }

  $effect(() => {
    // draw() reads audio.state.waveformData internally, so calling it here is
    // what tracks that reactive dependency and re-runs this effect on change.
    draw();
  });

  onMount(() => {
    if (canvas) {
      resizeObserver = new ResizeObserver(() => updateCanvasSize());
      resizeObserver.observe(canvas);
      updateCanvasSize();
    }
  });

  onDestroy(() => {
    resizeObserver?.disconnect();
  });
</script>

<canvas
  bind:this={canvas}
  width={canvasWidth}
  height={canvasHeight}
  class="waveform-canvas"
></canvas>

<style>
  .waveform-canvas {
    width: 100%;
    height: 80px;
    border-radius: var(--radius-md);
    display: block;
  }
</style>
