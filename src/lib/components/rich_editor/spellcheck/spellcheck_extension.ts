// Tiptap extension: in-app spellcheck via ProseMirror decorations.
//
// Renders an inline `.spellcheck-misspelled` decoration on every word the
// shared spellchecker singleton flags as unknown. Right-clicking a flagged
// word fires the host's `onContextMenu` callback so a custom menu can be
// shown; right-clicking a correctly-spelled word is passed through so the
// OS Cut/Copy/Paste menu still appears.
//
// PHI safety: this file performs no logging. Word content is never written
// to console or telemetry — only positions and decoration counts ever
// cross plugin boundaries.

import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import type { Transaction } from '@tiptap/pm/state';
import { Decoration, DecorationSet } from '@tiptap/pm/view';
import type { EditorView } from '@tiptap/pm/view';
import type { Node as ProseMirrorNode } from '@tiptap/pm/model';
import { getSpellchecker } from './spellchecker';

const SPELLCHECK_PLUGIN_KEY = new PluginKey<DecorationSet>('spellcheck');

/** Debounce timer for rescans after doc changes. Prevents per-keystroke
 *  full-document rescans which cause lag on long documents. */
const RESCAN_DEBOUNCE_MS = 250;

/** Module-level debounce state — one timer per active editor view. */
const rescanTimers = new Map<EditorView, ReturnType<typeof setTimeout>>();

/** Meta key set on transactions that should force the plugin to re-scan
 *  even when the doc did not change (e.g. after adding to user dict, after
 *  ignoring a word in-session, or after the dictionary finishes loading). */
const RESCAN_META = 'spellcheck-rescan';

export interface SpellcheckContextMenuRequest {
  word: string;
  from: number; // ProseMirror position of word start
  to: number;
  clientX: number;
  clientY: number;
}

export interface SpellcheckOptions {
  /** Called when the user right-clicks a misspelled word. The host renders
   *  the menu at the given client coordinates and calls back into the
   *  editor via the editor commands. */
  onContextMenu: (req: SpellcheckContextMenuRequest, view: EditorView) => void;
}

// Word boundary regex. \p{L} + \p{N} captures Unicode letters and numbers;
// apostrophes and hyphens are kept so contractions and hyphenated words
// stay as a single token. Stateful (`g` flag), so callers must reset
// `lastIndex` before use.
const WORD_RE = /[\p{L}\p{N}'-]+/gu;

/** Strip leading/trailing apostrophes and hyphens from a word token
 *  to avoid false positives from quotes and dashes adjacent to words. */
function cleanToken(raw: string): string {
  return raw.replace(/^['-]+|['-]+$/g, '');
}

/** Check if a word (possibly hyphenated) is correctly spelled.
 *  For hyphenated words like "lisinopril-hctz", checks each part
 *  individually — if ALL parts are correct, the whole word is correct. */
function checkWord(spell: ReturnType<typeof getSpellchecker>, word: string): boolean {
  // For hyphenated words, check each part.
  if (word.includes('-')) {
    const parts = word.split('-').filter((p) => p.length > 0);
    if (parts.length === 0) return true;
    // All parts must be correct for the compound to be correct.
    return parts.every((part) => spell.check(part));
  }
  return spell.check(word);
}

function scanDecorations(doc: ProseMirrorNode): DecorationSet {
  const spell = getSpellchecker();
  if (!spell.ready) return DecorationSet.empty;
  const decos: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (!node.isTextblock) return; // Only process textblock-level nodes
    const text = node.textContent;
    if (!text) return;
    // Build a position map: for each character index in the concatenated
    // text, what is the corresponding ProseMirror doc position?
    // We walk the textblock's children to build this map.
    const positions: number[] = [];
    let docPos = pos + 1; // +1 to skip into the textblock content
    let textIdx = 0;
    node.forEach((child) => {
      if (child.isText && child.text) {
        for (let i = 0; i < child.text.length; i++) {
          positions[textIdx] = docPos;
          textIdx++;
          docPos++;
        }
      } else {
        docPos += child.nodeSize;
      }
    });
    const actualLen = textIdx;

    WORD_RE.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = WORD_RE.exec(text)) !== null) {
      const raw = match[0];
      const word = cleanToken(raw);
      if (word.length < 2) continue;
      const matchStart = match.index;
      if (matchStart >= actualLen) break;
      if (checkWord(spell, word)) continue;
      // Calculate decoration positions using the position map.
      const leadingLen = raw.length - raw.replace(/^['-]+/, '').length;
      const wordStartIdx = matchStart + leadingLen;
      const wordEndIdx = wordStartIdx + word.length;
      if (wordStartIdx >= actualLen || wordEndIdx > actualLen + 1) continue;
      const start = positions[wordStartIdx] ?? pos + 1 + wordStartIdx;
      const end = (wordEndIdx <= actualLen ? positions[wordEndIdx - 1] : pos + 1 + actualLen) + 1;
      decos.push(
        Decoration.inline(start, end, { class: 'spellcheck-misspelled' }),
      );
    }
  });
  return DecorationSet.create(doc, decos);
}

// Module-level signal flipped when the dictionary finishes loading. The
// plugin's `apply` checks `DICT_JUST_LOADED()` on every transaction; the
// first transaction observed after load causes a full re-scan. This is the
// simplest cross-view mechanism — the plugin instance has no handle to the
// view from inside `state.init`, and we want every active editor to
// re-scan exactly once.
let DICT_LOADED = false;
let SEEN_LOADED = false;

/** Active editor views — tracked so `requestSpellcheckRescan` can be called
 *  without a specific view reference (e.g., from the DictionaryDialog in
 *  Settings, which doesn't have access to any editor). */
const activeViews = new Set<EditorView>();
function DICT_JUST_LOADED(): boolean {
  if (DICT_LOADED && !SEEN_LOADED) {
    SEEN_LOADED = true;
    return true;
  }
  return false;
}

function wordAtPos(
  view: EditorView,
  pos: number,
): { text: string; from: number; to: number } | null {
  const $pos = view.state.doc.resolve(pos);
  const node = $pos.parent;
  if (!node.isTextblock) return null;
  const text = node.textContent;
  const offsetInBlock = pos - $pos.start();
  WORD_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = WORD_RE.exec(text)) !== null) {
    const raw = match[0];
    const word = cleanToken(raw);
    const leadingLen = raw.length - raw.replace(/^['-]+/, '').length;
    const start = match.index + leadingLen;
    const end = start + word.length;
    if (offsetInBlock >= start && offsetInBlock <= end) {
      return {
        text: word,
        from: $pos.start() + start,
        to: $pos.start() + end,
      };
    }
  }
  return null;
}

export const Spellcheck = Extension.create<SpellcheckOptions>({
  name: 'spellcheck',

  addOptions(): SpellcheckOptions {
    return { onContextMenu: () => {} };
  },

  addProseMirrorPlugins() {
    const opts = this.options;
    // Kick off async dictionary load and flip the module-level signal when
    // ready so the next transaction re-scans.
    const spell = getSpellchecker();
    if (!spell.ready) {
      spell.load().then(() => {
        DICT_LOADED = true;
        SEEN_LOADED = false;
      });
    } else {
      DICT_LOADED = true;
    }

    return [
      new Plugin<DecorationSet>({
        key: SPELLCHECK_PLUGIN_KEY,
        view(editorView) {
          activeViews.add(editorView);
          return {
            destroy() {
              activeViews.delete(editorView);
            },
          };
        },
        state: {
          init: (_cfg, state) => scanDecorations(state.doc),
          apply(tr: Transaction, oldSet, _oldState, newState) {
            // Immediate rescan for non-doc-change signals (dict load, manual rescan).
            if (DICT_JUST_LOADED() || tr.getMeta(RESCAN_META)) {
              return scanDecorations(newState.doc);
            }
            if (tr.docChanged) {
              // Debounce the rescan — map existing decorations for immediate
              // position updates, then schedule a full rescan after the user
              // stops typing. This prevents O(document) scans per keystroke.
              const mapped = oldSet.map(tr.mapping, tr.doc);
              // Clear any pending debounced rescans.
              for (const t of rescanTimers.values()) clearTimeout(t);
              rescanTimers.clear();
              // Schedule a full rescan across all active views after debounce.
              const timer = setTimeout(() => {
                rescanTimers.clear();
                for (const v of activeViews) {
                  try {
                    v.dispatch(v.state.tr.setMeta(RESCAN_META, true));
                  } catch { /* view may have been destroyed */ }
                }
              }, RESCAN_DEBOUNCE_MS);
              // Track the timer for cleanup (use first active view as key).
              for (const v of activeViews) {
                rescanTimers.set(v, timer);
                break;
              }
              return mapped;
            }
            return oldSet.map(tr.mapping, tr.doc);
          },
        },
        props: {
          decorations(state) {
            return SPELLCHECK_PLUGIN_KEY.getState(state) ?? DecorationSet.empty;
          },
          handleDOMEvents: {
            contextmenu(view, event) {
              const me = event as MouseEvent;
              const pos = view.posAtCoords({
                left: me.clientX,
                top: me.clientY,
              });
              if (!pos) return false;
              const word = wordAtPos(view, pos.pos);
              if (!word) return false;
              const spell = getSpellchecker();
              if (spell.check(word.text)) return false; // spelled correctly — defer to OS menu
              opts.onContextMenu(
                {
                  word: word.text,
                  from: word.from,
                  to: word.to,
                  clientX: me.clientX,
                  clientY: me.clientY,
                },
                view,
              );
              event.preventDefault();
              return true;
            },
          },
        },
      }),
    ];
  },
});

/** Dispatch a no-op transaction tagged with the rescan meta so the
 *  spellcheck plugin re-runs `scanDecorations`. Use after mutating the
 *  user dictionary or session-ignore set from outside the editor.
 *
 *  If `view` is provided, rescans only that view. Otherwise rescans all
 *  active editor views (tracked via the plugin's `view` lifecycle hook). */
export function requestSpellcheckRescan(view?: EditorView): void {
  if (view) {
    view.dispatch(view.state.tr.setMeta(RESCAN_META, true));
  } else {
    for (const v of activeViews) {
      try {
        v.dispatch(v.state.tr.setMeta(RESCAN_META, true));
      } catch (e) {
        console.error('spellcheck rescan failed for view:', e);
      }
    }
  }
}
