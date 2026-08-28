/**
 * Single source of truth for the in-app Terms of Service text.
 *
 * The repo-root TERMS_OF_SERVICE.md stays canonical — this import pulls it
 * in raw at build time (Vite `?raw`), so the document is never copied into
 * a second file that could drift. The document is intentionally plain
 * ASCII text (not markdown); renderers should display it in a scrollable
 * `white-space: pre-wrap` block.
 */
import raw from '../../TERMS_OF_SERVICE.md?raw';

export const TERMS_OF_SERVICE_TEXT: string = raw;
