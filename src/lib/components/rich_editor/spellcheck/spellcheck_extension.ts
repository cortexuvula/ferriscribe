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

function scanDecorations(doc: ProseMirrorNode): DecorationSet {
  const spell = getSpellchecker();
  if (!spell.ready) return DecorationSet.empty;
  const decos: Decoration[] = [];
  doc.descendants((node, pos) => {
    if (!node.isText || !node.text) return;
    const text = node.text;
    WORD_RE.lastIndex = 0;
    let match: RegExpExecArray | null;
    while ((match = WORD_RE.exec(text)) !== null) {
      const word = match[0];
      if (word.length < 2) continue;
      if (spell.check(word)) continue;
      const start = pos + match.index;
      const end = start + word.length;
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
    const start = match.index;
    const end = start + match[0].length;
    if (offsetInBlock >= start && offsetInBlock <= end) {
      return {
        text: match[0],
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
        state: {
          init: (_cfg, state) => scanDecorations(state.doc),
          apply(tr: Transaction, oldSet, _oldState, newState) {
            if (
              tr.docChanged ||
              DICT_JUST_LOADED() ||
              tr.getMeta(RESCAN_META)
            ) {
              return scanDecorations(newState.doc);
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
 *  user dictionary or session-ignore set from outside the editor. */
export function requestSpellcheckRescan(view: EditorView): void {
  view.dispatch(view.state.tr.setMeta(RESCAN_META, true));
}
