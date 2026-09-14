// #108 finding (c) / contract line H: badge text and the unassigned
// boundary must meet WCAG 4.5:1. This gate recomputes every ratio from the
// component's OWN CSS declarations (Vite ?raw import) — there is no
// parallel colour table to drift out of sync. Change a colour in
// TranscriptView.svelte and this test re-evaluates it; change a colour HERE
// and nothing happens (there are no colours here to change).
//
// Method — mirrors the review's contrast.json measurement:
//   1. badge text colour  = the --slot-text value for slot N / theme
//   2. badge background   = palette[N].bg (a 12%-alpha tint) alpha-
//                            composited over each production surface
//                            (--bg-primary, --bg-secondary) for the theme
//   3. contrast ratio     = WCAG relative luminance formula
// The ratio must clear 4.5:1 for BOTH surfaces in BOTH themes. The chosen
// values actually clear 5:1 worst-case, giving headroom for user font
// smoothing / font-weight rendering variance.
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import componentSource from './TranscriptView.svelte?raw';

// app.css?raw resolves to an EMPTY string under this Vite/vitest combination
// (verified empirically), which silently turned every contrast assertion
// into garbage. Read the file directly instead — same bytes, no transform.
const appCss = readFileSync(new URL('../../app.css', import.meta.url), 'utf8');

// ── parsing helpers ─────────────────────────────────────────────────────────

function cssVar(name: string, theme: 'light' | 'dark'): string {
  // app.css declares the light block as a multi-line selector
  // (`:root,\n[data-theme='light'] {`), so match on the attribute selector
  // alone and take the first (light) block for 'light'.
  const pattern =
    theme === 'light'
      ? /\[data-theme='light'\]\s*\{([\s\S]*?)\}/m
      : /\[data-theme='dark'\]\s*\{([\s\S]*?)\}/m;
  const block = appCss.match(pattern);
  if (!block) throw new Error(`theme block not found: ${theme}`);
  const m = block[1].match(new RegExp(`--${name}:\\s*([^;]+);`));
  if (!m) throw new Error(`--${name} not found for ${theme}`);
  return m[1].trim();
}

function slotTextColor(slot: number, theme: 'light' | 'dark'): string {
  // Line-based: light slot rules are `  .slot-N { --slot-text: …; }` inside
  // the component's scoped style; dark rules are prefixed
  // `:global([data-theme='dark']) .slot-N { … }`.
  const lines = componentSource.split('\n');
  const decl =
    theme === 'light'
      ? lines.find((l) => new RegExp(`^\\s*\\.slot-${slot}\\s*\\{`).test(l))
      : lines.find((l) => new RegExp(`:global\\(\\[data-theme='dark'\\]\\)\\s*\\.slot-${slot}\\s*\\{`).test(l));
  const m = decl?.match(/--slot-text:\s*([^;]+);/);
  if (!m) throw new Error(`--slot-text for slot ${slot} (${theme}) not found in component CSS`);
  return m[1].trim();
}

function paletteBg(slot: number): { base: string; alpha: number } {
  const m = componentSource.match(new RegExp(`bg:\\s*'rgba\\((\\d+),\\s*(\\d+),\\s*(\\d+),\\s*([\\d.]+)\\)'`, 'g'));
  if (!m) throw new Error('palette bg entries not found');
  const entry = m[slot].match(/rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)/)!;
  return {
    base: `#${[1, 2, 3].map((i) => Number(entry[i]).toString(16).padStart(2, '0')).join('')}`,
    alpha: Number(entry[4]),
  };
}

// ── WCAG math (srgb) ────────────────────────────────────────────────────────

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace('#', '');
  return [parseInt(h.slice(0, 2), 16), parseInt(h.slice(2, 4), 16), parseInt(h.slice(4, 6), 16)];
}

function composite(fg: string, alpha: number, bg: string): [number, number, number] {
  const f = hexToRgb(fg);
  const b = hexToRgb(bg);
  return [0, 1, 2].map((i) => alpha * f[i] + (1 - alpha) * b[i]) as [number, number, number];
}

function relativeLuminance(rgb: [number, number, number]): number {
  const channel = (c: number) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = rgb.map(channel) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrastRatio(a: [number, number, number], b: [number, number, number]): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const [hi, lo] = la > lb ? [la, lb] : [lb, la];
  return (hi + 0.05) / (lo + 0.05);
}

// ── the gate ────────────────────────────────────────────────────────────────

const SLOTS = [0, 1, 2, 3, 4, 5, 6, 7];
const THEMES = ['light', 'dark'] as const;
const SURFACE_VARS = ['bg-primary', 'bg-secondary'] as const;

describe('TranscriptView badge contrast (#108 finding c, contract line H)', () => {
  for (const theme of THEMES) {
    for (const slot of SLOTS) {
      it(`slot ${slot} text ≥4.5:1 on composited badge tint (${theme}, both surfaces)`, () => {
        const text = slotTextColor(slot, theme);
        expect(text).toMatch(/^#[0-9a-f]{6}$/i);
        const { base, alpha } = paletteBg(slot);
        for (const surfaceVar of SURFACE_VARS) {
          const surface = cssVar(surfaceVar, theme);
          const bg = composite(base, alpha, surface);
          const ratio = contrastRatio(hexToRgb(text), bg);
          // Report the actual number in the assertion message for triage.
          expect(
            ratio,
            `slot ${slot} ${theme} on --${surfaceVar}: ${ratio.toFixed(2)}:1 (text ${text} on tint ${base}@${alpha} over ${surface})`,
          ).toBeGreaterThanOrEqual(4.5);
        }
      });
    }
  }

  it('unassigned rail uses --text-secondary at ≥4.5:1 in both themes (strengthened from var(--border))', () => {
    const m = componentSource.match(/\.speaker-section\.unlabeled\s*\{[\s\S]*?border-left:\s*([^;]+);/);
    expect(m).toBeTruthy();
    expect(m![1]).toContain('var(--text-secondary)');
    for (const theme of THEMES) {
      const rail = cssVar('text-secondary', theme);
      const ratio = Math.min(
        ...SURFACE_VARS.map((v) => contrastRatio(hexToRgb(rail), hexToRgb(cssVar(v, theme)))),
      );
      expect(ratio, `rail ${theme}: ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('unassigned heading is body-adjacent (13px) and full-contrast text', () => {
    const size = componentSource.match(/\.unlabeled-heading\s*\{[\s\S]*?font-size:\s*([^;]+);/)?.[1];
    const color = componentSource.match(/\.unlabeled-heading\s*\{[\s\S]*?color:\s*([^;]+);/)?.[1];
    expect(size?.trim()).toBe('13px');
    expect(color?.trim()).toBe('var(--text-primary)');
  });

  it('badge text is set by theme-aware slot classes, never by an inline palette colour', () => {
    // The regression this guards: re-wiring the old inline
    // `color: {colors.text}` (the 1.82–3.42:1 light-mode failure).
    expect(componentSource).not.toMatch(/color:\s*\{colors\.text\}/);
    // The badge must carry a slot-N class for the CSS custom property to
    // resolve, with --text-primary fallback for a missing slot.
    expect(componentSource).toMatch(/class="speaker-badge slot-\{slot\}"/);
    expect(componentSource).toMatch(/--slot-text,\s*var\(--text-primary\)/);
  });
});
