import { C as Fragment, D as Schema, E as NodeRange, O as Slice, S as DOMSerializer, T as Node$1, _ as findWrapping, a as PluginKey, b as replaceStep, c as TextSelection, d as ReplaceAroundStep, f as ReplaceStep, h as canSplit, i as Plugin, m as canJoin, n as EditorState, o as Selection, p as Transform, r as NodeSelection, t as AllSelection, v as joinPoint, x as DOMParser, y as liftTarget } from "./dist-D4JOGMZD.js";
import "./@tiptap_pm_state.js";
import { EditorView } from "./@tiptap_pm_view.js";
//#region ../../node_modules/w3c-keyname/index.js
var base = {
	8: "Backspace",
	9: "Tab",
	10: "Enter",
	12: "NumLock",
	13: "Enter",
	16: "Shift",
	17: "Control",
	18: "Alt",
	20: "CapsLock",
	27: "Escape",
	32: " ",
	33: "PageUp",
	34: "PageDown",
	35: "End",
	36: "Home",
	37: "ArrowLeft",
	38: "ArrowUp",
	39: "ArrowRight",
	40: "ArrowDown",
	44: "PrintScreen",
	45: "Insert",
	46: "Delete",
	59: ";",
	61: "=",
	91: "Meta",
	92: "Meta",
	106: "*",
	107: "+",
	108: ",",
	109: "-",
	110: ".",
	111: "/",
	144: "NumLock",
	145: "ScrollLock",
	160: "Shift",
	161: "Shift",
	162: "Control",
	163: "Control",
	164: "Alt",
	165: "Alt",
	173: "-",
	186: ";",
	187: "=",
	188: ",",
	189: "-",
	190: ".",
	191: "/",
	192: "`",
	219: "[",
	220: "\\",
	221: "]",
	222: "'"
};
var shift = {
	48: ")",
	49: "!",
	50: "@",
	51: "#",
	52: "$",
	53: "%",
	54: "^",
	55: "&",
	56: "*",
	57: "(",
	59: ":",
	61: "+",
	173: "_",
	186: ":",
	187: "+",
	188: "<",
	189: "_",
	190: ">",
	191: "?",
	192: "~",
	219: "{",
	220: "|",
	221: "}",
	222: "\""
};
var mac$1 = typeof navigator != "undefined" && /Mac/.test(navigator.platform);
var ie = typeof navigator != "undefined" && /MSIE \d|Trident\/(?:[7-9]|\d{2,})\..*rv:(\d+)/.exec(navigator.userAgent);
for (var i = 0; i < 10; i++) base[48 + i] = base[96 + i] = String(i);
for (var i = 1; i <= 24; i++) base[i + 111] = "F" + i;
for (var i = 65; i <= 90; i++) {
	base[i] = String.fromCharCode(i + 32);
	shift[i] = String.fromCharCode(i);
}
for (var code in base) if (!shift.hasOwnProperty(code)) shift[code] = base[code];
function keyName(event) {
	var name = !(mac$1 && event.metaKey && event.shiftKey && !event.ctrlKey && !event.altKey || ie && event.shiftKey && event.key && event.key.length == 1 || event.key == "Unidentified") && event.key || (event.shiftKey ? shift : base)[event.keyCode] || event.key || "Unidentified";
	if (name == "Esc") name = "Escape";
	if (name == "Del") name = "Delete";
	if (name == "Left") name = "ArrowLeft";
	if (name == "Up") name = "ArrowUp";
	if (name == "Right") name = "ArrowRight";
	if (name == "Down") name = "ArrowDown";
	return name;
}
//#endregion
//#region ../../node_modules/prosemirror-keymap/dist/index.js
var mac = typeof navigator != "undefined" && /Mac|iP(hone|[oa]d)/.test(navigator.platform);
var windows = typeof navigator != "undefined" && /Win/.test(navigator.platform);
function normalizeKeyName$1(name) {
	let parts = name.split(/-(?!$)/), result = parts[parts.length - 1];
	if (result == "Space") result = " ";
	let alt, ctrl, shift, meta;
	for (let i = 0; i < parts.length - 1; i++) {
		let mod = parts[i];
		if (/^(cmd|meta|m)$/i.test(mod)) meta = true;
		else if (/^a(lt)?$/i.test(mod)) alt = true;
		else if (/^(c|ctrl|control)$/i.test(mod)) ctrl = true;
		else if (/^s(hift)?$/i.test(mod)) shift = true;
		else if (/^mod$/i.test(mod)) {
			if (mac) meta = true;
			else ctrl = true;
		} else throw new Error("Unrecognized modifier name: " + mod);
	}
	if (alt) result = "Alt-" + result;
	if (ctrl) result = "Ctrl-" + result;
	if (meta) result = "Meta-" + result;
	if (shift) result = "Shift-" + result;
	return result;
}
function normalize(map) {
	let copy = Object.create(null);
	for (let prop in map) copy[normalizeKeyName$1(prop)] = map[prop];
	return copy;
}
function modifiers(name, event, shift = true) {
	if (event.altKey) name = "Alt-" + name;
	if (event.ctrlKey) name = "Ctrl-" + name;
	if (event.metaKey) name = "Meta-" + name;
	if (shift && event.shiftKey) name = "Shift-" + name;
	return name;
}
/**
Create a keymap plugin for the given set of bindings.

Bindings should map key names to [command](https://prosemirror.net/docs/ref/#commands)-style
functions, which will be called with `(EditorState, dispatch,
EditorView)` arguments, and should return true when they've handled
the key. Note that the view argument isn't part of the command
protocol, but can be used as an escape hatch if a binding needs to
directly interact with the UI.

Key names may be strings like `"Shift-Ctrl-Enter"`—a key
identifier prefixed with zero or more modifiers. Key identifiers
are based on the strings that can appear in
[`KeyEvent.key`](https:developer.mozilla.org/en-US/docs/Web/API/KeyboardEvent/key).
Use lowercase letters to refer to letter keys (or uppercase letters
if you want shift to be held). You may use `"Space"` as an alias
for the `" "` name.

Modifiers can be given in any order. `Shift-` (or `s-`), `Alt-` (or
`a-`), `Ctrl-` (or `c-` or `Control-`) and `Cmd-` (or `m-` or
`Meta-`) are recognized. For characters that are created by holding
shift, the `Shift-` prefix is implied, and should not be added
explicitly.

You can use `Mod-` as a shorthand for `Cmd-` on Mac and `Ctrl-` on
other platforms.

You can add multiple keymap plugins to an editor. The order in
which they appear determines their precedence (the ones early in
the array get to dispatch first).
*/
function keymap(bindings) {
	return new Plugin({ props: { handleKeyDown: keydownHandler(bindings) } });
}
/**
Given a set of bindings (using the same format as
[`keymap`](https://prosemirror.net/docs/ref/#keymap.keymap)), return a [keydown
handler](https://prosemirror.net/docs/ref/#view.EditorProps.handleKeyDown) that handles them.
*/
function keydownHandler(bindings) {
	let map = normalize(bindings);
	return function(view, event) {
		let name = keyName(event), baseName, direct = map[modifiers(name, event)];
		if (direct && direct(view.state, view.dispatch, view)) return true;
		if (name.length == 1 && name != " ") {
			if (event.shiftKey) {
				let noShift = map[modifiers(name, event, false)];
				if (noShift && noShift(view.state, view.dispatch, view)) return true;
			}
			if ((event.altKey || event.metaKey || event.ctrlKey) && !(windows && event.ctrlKey && event.altKey) && (baseName = base[event.keyCode]) && baseName != name) {
				let fromCode = map[modifiers(baseName, event)];
				if (fromCode && fromCode(view.state, view.dispatch, view)) return true;
			}
		}
		return false;
	};
}
//#endregion
//#region ../../node_modules/prosemirror-commands/dist/index.js
/**
Delete the selection, if there is one.
*/
var deleteSelection$1 = (state, dispatch) => {
	if (state.selection.empty) return false;
	if (dispatch) dispatch(state.tr.deleteSelection().scrollIntoView());
	return true;
};
function atBlockStart(state, view) {
	let { $cursor } = state.selection;
	if (!$cursor || (view ? !view.endOfTextblock("backward", state) : $cursor.parentOffset > 0)) return null;
	return $cursor;
}
/**
If the selection is empty and at the start of a textblock, try to
reduce the distance between that block and the one before it—if
there's a block directly before it that can be joined, join them.
If not, try to move the selected block closer to the next one in
the document structure by lifting it out of its parent or moving it
into a parent of the previous block. Will use the view for accurate
(bidi-aware) start-of-textblock detection if given.
*/
var joinBackward$1 = (state, dispatch, view) => {
	let $cursor = atBlockStart(state, view);
	if (!$cursor) return false;
	let $cut = findCutBefore($cursor);
	if (!$cut) {
		let range = $cursor.blockRange(), target = range && liftTarget(range);
		if (target == null) return false;
		if (dispatch) dispatch(state.tr.lift(range, target).scrollIntoView());
		return true;
	}
	let before = $cut.nodeBefore;
	if (deleteBarrier(state, $cut, dispatch, -1)) return true;
	if ($cursor.parent.content.size == 0 && (textblockAt(before, "end") || NodeSelection.isSelectable(before))) for (let depth = $cursor.depth;; depth--) {
		let delStep = replaceStep(state.doc, $cursor.before(depth), $cursor.after(depth), Slice.empty);
		if (delStep && delStep.slice.size < delStep.to - delStep.from) {
			if (dispatch) {
				let tr = state.tr.step(delStep);
				tr.setSelection(textblockAt(before, "end") ? Selection.findFrom(tr.doc.resolve(tr.mapping.map($cut.pos, -1)), -1) : NodeSelection.create(tr.doc, $cut.pos - before.nodeSize));
				dispatch(tr.scrollIntoView());
			}
			return true;
		}
		if (depth == 1 || $cursor.node(depth - 1).childCount > 1) break;
	}
	if (before.isAtom && $cut.depth == $cursor.depth - 1) {
		if (dispatch) dispatch(state.tr.delete($cut.pos - before.nodeSize, $cut.pos).scrollIntoView());
		return true;
	}
	return false;
};
/**
A more limited form of [`joinBackward`](https://prosemirror.net/docs/ref/#commands.joinBackward)
that only tries to join the current textblock to the one before
it, if the cursor is at the start of a textblock.
*/
var joinTextblockBackward$1 = (state, dispatch, view) => {
	let $cursor = atBlockStart(state, view);
	if (!$cursor) return false;
	let $cut = findCutBefore($cursor);
	return $cut ? joinTextblocksAround(state, $cut, dispatch) : false;
};
/**
A more limited form of [`joinForward`](https://prosemirror.net/docs/ref/#commands.joinForward)
that only tries to join the current textblock to the one after
it, if the cursor is at the end of a textblock.
*/
var joinTextblockForward$1 = (state, dispatch, view) => {
	let $cursor = atBlockEnd(state, view);
	if (!$cursor) return false;
	let $cut = findCutAfter($cursor);
	return $cut ? joinTextblocksAround(state, $cut, dispatch) : false;
};
function joinTextblocksAround(state, $cut, dispatch) {
	let beforeText = $cut.nodeBefore, beforePos = $cut.pos - 1;
	for (; !beforeText.isTextblock; beforePos--) {
		if (beforeText.type.spec.isolating) return false;
		let child = beforeText.lastChild;
		if (!child) return false;
		beforeText = child;
	}
	let afterText = $cut.nodeAfter, afterPos = $cut.pos + 1;
	for (; !afterText.isTextblock; afterPos++) {
		if (afterText.type.spec.isolating) return false;
		let child = afterText.firstChild;
		if (!child) return false;
		afterText = child;
	}
	let step = replaceStep(state.doc, beforePos, afterPos, Slice.empty);
	if (!step || step.from != beforePos || step instanceof ReplaceStep && step.slice.size >= afterPos - beforePos) return false;
	if (dispatch) {
		let tr = state.tr.step(step);
		tr.setSelection(TextSelection.create(tr.doc, beforePos));
		dispatch(tr.scrollIntoView());
	}
	return true;
}
function textblockAt(node, side, only = false) {
	for (let scan = node; scan; scan = side == "start" ? scan.firstChild : scan.lastChild) {
		if (scan.isTextblock) return true;
		if (only && scan.childCount != 1) return false;
	}
	return false;
}
/**
When the selection is empty and at the start of a textblock, select
the node before that textblock, if possible. This is intended to be
bound to keys like backspace, after
[`joinBackward`](https://prosemirror.net/docs/ref/#commands.joinBackward) or other deleting
commands, as a fall-back behavior when the schema doesn't allow
deletion at the selected point.
*/
var selectNodeBackward$1 = (state, dispatch, view) => {
	let { $head, empty } = state.selection, $cut = $head;
	if (!empty) return false;
	if ($head.parent.isTextblock) {
		if (view ? !view.endOfTextblock("backward", state) : $head.parentOffset > 0) return false;
		$cut = findCutBefore($head);
	}
	let node = $cut && $cut.nodeBefore;
	if (!node || !NodeSelection.isSelectable(node)) return false;
	if (dispatch) dispatch(state.tr.setSelection(NodeSelection.create(state.doc, $cut.pos - node.nodeSize)).scrollIntoView());
	return true;
};
function findCutBefore($pos) {
	if (!$pos.parent.type.spec.isolating) for (let i = $pos.depth - 1; i >= 0; i--) {
		if ($pos.index(i) > 0) return $pos.doc.resolve($pos.before(i + 1));
		if ($pos.node(i).type.spec.isolating) break;
	}
	return null;
}
function atBlockEnd(state, view) {
	let { $cursor } = state.selection;
	if (!$cursor || (view ? !view.endOfTextblock("forward", state) : $cursor.parentOffset < $cursor.parent.content.size)) return null;
	return $cursor;
}
/**
If the selection is empty and the cursor is at the end of a
textblock, try to reduce or remove the boundary between that block
and the one after it, either by joining them or by moving the other
block closer to this one in the tree structure. Will use the view
for accurate start-of-textblock detection if given.
*/
var joinForward$1 = (state, dispatch, view) => {
	let $cursor = atBlockEnd(state, view);
	if (!$cursor) return false;
	let $cut = findCutAfter($cursor);
	if (!$cut) return false;
	let after = $cut.nodeAfter;
	if (deleteBarrier(state, $cut, dispatch, 1)) return true;
	if ($cursor.parent.content.size == 0 && (textblockAt(after, "start") || NodeSelection.isSelectable(after))) {
		let delStep = replaceStep(state.doc, $cursor.before(), $cursor.after(), Slice.empty);
		if (delStep && delStep.slice.size < delStep.to - delStep.from) {
			if (dispatch) {
				let tr = state.tr.step(delStep);
				tr.setSelection(textblockAt(after, "start") ? Selection.findFrom(tr.doc.resolve(tr.mapping.map($cut.pos)), 1) : NodeSelection.create(tr.doc, tr.mapping.map($cut.pos)));
				dispatch(tr.scrollIntoView());
			}
			return true;
		}
	}
	if (after.isAtom && $cut.depth == $cursor.depth - 1) {
		if (dispatch) dispatch(state.tr.delete($cut.pos, $cut.pos + after.nodeSize).scrollIntoView());
		return true;
	}
	return false;
};
/**
When the selection is empty and at the end of a textblock, select
the node coming after that textblock, if possible. This is intended
to be bound to keys like delete, after
[`joinForward`](https://prosemirror.net/docs/ref/#commands.joinForward) and similar deleting
commands, to provide a fall-back behavior when the schema doesn't
allow deletion at the selected point.
*/
var selectNodeForward$1 = (state, dispatch, view) => {
	let { $head, empty } = state.selection, $cut = $head;
	if (!empty) return false;
	if ($head.parent.isTextblock) {
		if (view ? !view.endOfTextblock("forward", state) : $head.parentOffset < $head.parent.content.size) return false;
		$cut = findCutAfter($head);
	}
	let node = $cut && $cut.nodeAfter;
	if (!node || !NodeSelection.isSelectable(node)) return false;
	if (dispatch) dispatch(state.tr.setSelection(NodeSelection.create(state.doc, $cut.pos)).scrollIntoView());
	return true;
};
function findCutAfter($pos) {
	if (!$pos.parent.type.spec.isolating) for (let i = $pos.depth - 1; i >= 0; i--) {
		let parent = $pos.node(i);
		if ($pos.index(i) + 1 < parent.childCount) return $pos.doc.resolve($pos.after(i + 1));
		if (parent.type.spec.isolating) break;
	}
	return null;
}
/**
Join the selected block or, if there is a text selection, the
closest ancestor block of the selection that can be joined, with
the sibling above it.
*/
var joinUp$1 = (state, dispatch) => {
	let sel = state.selection, nodeSel = sel instanceof NodeSelection, point;
	if (nodeSel) {
		if (sel.node.isTextblock || !canJoin(state.doc, sel.from)) return false;
		point = sel.from;
	} else {
		point = joinPoint(state.doc, sel.from, -1);
		if (point == null) return false;
	}
	if (dispatch) {
		let tr = state.tr.join(point);
		if (nodeSel) tr.setSelection(NodeSelection.create(tr.doc, point - state.doc.resolve(point).nodeBefore.nodeSize));
		dispatch(tr.scrollIntoView());
	}
	return true;
};
/**
Join the selected block, or the closest ancestor of the selection
that can be joined, with the sibling after it.
*/
var joinDown$1 = (state, dispatch) => {
	let sel = state.selection, point;
	if (sel instanceof NodeSelection) {
		if (sel.node.isTextblock || !canJoin(state.doc, sel.to)) return false;
		point = sel.to;
	} else {
		point = joinPoint(state.doc, sel.to, 1);
		if (point == null) return false;
	}
	if (dispatch) dispatch(state.tr.join(point).scrollIntoView());
	return true;
};
/**
Lift the selected block, or the closest ancestor block of the
selection that can be lifted, out of its parent node.
*/
var lift$1 = (state, dispatch) => {
	let { $from, $to } = state.selection;
	let range = $from.blockRange($to), target = range && liftTarget(range);
	if (target == null) return false;
	if (dispatch) dispatch(state.tr.lift(range, target).scrollIntoView());
	return true;
};
/**
If the selection is in a node whose type has a truthy
[`code`](https://prosemirror.net/docs/ref/#model.NodeSpec.code) property in its spec, replace the
selection with a newline character.
*/
var newlineInCode$1 = (state, dispatch) => {
	let { $head, $anchor } = state.selection;
	if (!$head.parent.type.spec.code || !$head.sameParent($anchor)) return false;
	if (dispatch) dispatch(state.tr.insertText("\n").scrollIntoView());
	return true;
};
function defaultBlockAt$1(match) {
	for (let i = 0; i < match.edgeCount; i++) {
		let { type } = match.edge(i);
		if (type.isTextblock && !type.hasRequiredAttrs()) return type;
	}
	return null;
}
/**
When the selection is in a node with a truthy
[`code`](https://prosemirror.net/docs/ref/#model.NodeSpec.code) property in its spec, create a
default block after the code block, and move the cursor there.
*/
var exitCode$1 = (state, dispatch) => {
	let { $head, $anchor } = state.selection;
	if (!$head.parent.type.spec.code || !$head.sameParent($anchor)) return false;
	let above = $head.node(-1), after = $head.indexAfter(-1), type = defaultBlockAt$1(above.contentMatchAt(after));
	if (!type || !above.canReplaceWith(after, after, type)) return false;
	if (dispatch) {
		let pos = $head.after(), tr = state.tr.replaceWith(pos, pos, type.createAndFill());
		tr.setSelection(Selection.near(tr.doc.resolve(pos), 1));
		dispatch(tr.scrollIntoView());
	}
	return true;
};
/**
If a block node is selected, create an empty paragraph before (if
it is its parent's first child) or after it.
*/
var createParagraphNear$1 = (state, dispatch) => {
	let sel = state.selection, { $from, $to } = sel;
	if (sel instanceof AllSelection || $from.parent.inlineContent || $to.parent.inlineContent) return false;
	let type = defaultBlockAt$1($to.parent.contentMatchAt($to.indexAfter()));
	if (!type || !type.isTextblock) return false;
	if (dispatch) {
		let side = (!$from.parentOffset && $to.index() < $to.parent.childCount ? $from : $to).pos;
		let tr = state.tr.insert(side, type.createAndFill());
		tr.setSelection(TextSelection.create(tr.doc, side + 1));
		dispatch(tr.scrollIntoView());
	}
	return true;
};
/**
If the cursor is in an empty textblock that can be lifted, lift the
block.
*/
var liftEmptyBlock$1 = (state, dispatch) => {
	let { $cursor } = state.selection;
	if (!$cursor || $cursor.parent.content.size) return false;
	if ($cursor.depth > 1 && $cursor.after() != $cursor.end(-1)) {
		let before = $cursor.before();
		if (canSplit(state.doc, before)) {
			if (dispatch) dispatch(state.tr.split(before).scrollIntoView());
			return true;
		}
	}
	let range = $cursor.blockRange(), target = range && liftTarget(range);
	if (target == null) return false;
	if (dispatch) dispatch(state.tr.lift(range, target).scrollIntoView());
	return true;
};
/**
Create a variant of [`splitBlock`](https://prosemirror.net/docs/ref/#commands.splitBlock) that uses
a custom function to determine the type of the newly split off block.
*/
function splitBlockAs(splitNode) {
	return (state, dispatch) => {
		let { $from, $to } = state.selection;
		if (state.selection instanceof NodeSelection && state.selection.node.isBlock) {
			if (!$from.parentOffset || !canSplit(state.doc, $from.pos)) return false;
			if (dispatch) dispatch(state.tr.split($from.pos).scrollIntoView());
			return true;
		}
		if (!$from.depth) return false;
		let types = [];
		let splitDepth, deflt, atEnd = false, atStart = false;
		for (let d = $from.depth;; d--) if ($from.node(d).isBlock) {
			atEnd = $from.end(d) == $from.pos + ($from.depth - d);
			atStart = $from.start(d) == $from.pos - ($from.depth - d);
			deflt = defaultBlockAt$1($from.node(d - 1).contentMatchAt($from.indexAfter(d - 1)));
			let splitType = splitNode && splitNode($to.parent, atEnd, $from);
			types.unshift(splitType || (atEnd && deflt ? { type: deflt } : null));
			splitDepth = d;
			break;
		} else {
			if (d == 1) return false;
			types.unshift(null);
		}
		let tr = state.tr;
		if (state.selection instanceof TextSelection || state.selection instanceof AllSelection) tr.deleteSelection();
		let splitPos = tr.mapping.map($from.pos);
		let can = canSplit(tr.doc, splitPos, types.length, types);
		if (!can) {
			types[0] = deflt ? { type: deflt } : null;
			can = canSplit(tr.doc, splitPos, types.length, types);
		}
		if (!can) return false;
		tr.split(splitPos, types.length, types);
		if (!atEnd && atStart && $from.node(splitDepth).type != deflt) {
			let first = tr.mapping.map($from.before(splitDepth)), $first = tr.doc.resolve(first);
			if (deflt && $from.node(splitDepth - 1).canReplaceWith($first.index(), $first.index() + 1, deflt)) tr.setNodeMarkup(tr.mapping.map($from.before(splitDepth)), deflt);
		}
		if (dispatch) dispatch(tr.scrollIntoView());
		return true;
	};
}
/**
Split the parent block of the selection. If the selection is a text
selection, also delete its content.
*/
var splitBlock$1 = splitBlockAs();
/**
Move the selection to the node wrapping the current selection, if
any. (Will not select the document node.)
*/
var selectParentNode$1 = (state, dispatch) => {
	let { $from, to } = state.selection, pos;
	let same = $from.sharedDepth(to);
	if (same == 0) return false;
	pos = $from.before(same);
	if (dispatch) dispatch(state.tr.setSelection(NodeSelection.create(state.doc, pos)));
	return true;
};
/**
Select the whole document.
*/
var selectAll$1 = (state, dispatch) => {
	if (dispatch) dispatch(state.tr.setSelection(new AllSelection(state.doc)));
	return true;
};
function joinMaybeClear(state, $pos, dispatch) {
	let before = $pos.nodeBefore, after = $pos.nodeAfter, index = $pos.index();
	if (!before || !after || !before.type.compatibleContent(after.type)) return false;
	if (!before.content.size && $pos.parent.canReplace(index - 1, index)) {
		if (dispatch) dispatch(state.tr.delete($pos.pos - before.nodeSize, $pos.pos).scrollIntoView());
		return true;
	}
	if (!$pos.parent.canReplace(index, index + 1) || !(after.isTextblock || canJoin(state.doc, $pos.pos))) return false;
	if (dispatch) dispatch(state.tr.join($pos.pos).scrollIntoView());
	return true;
}
function deleteBarrier(state, $cut, dispatch, dir) {
	let before = $cut.nodeBefore, after = $cut.nodeAfter, conn, match;
	let isolated = before.type.spec.isolating || after.type.spec.isolating;
	if (!isolated && joinMaybeClear(state, $cut, dispatch)) return true;
	let canDelAfter = !isolated && $cut.parent.canReplace($cut.index(), $cut.index() + 1);
	if (canDelAfter && (conn = (match = before.contentMatchAt(before.childCount)).findWrapping(after.type)) && match.matchType(conn[0] || after.type).validEnd) {
		if (dispatch) {
			let end = $cut.pos + after.nodeSize, wrap = Fragment.empty;
			for (let i = conn.length - 1; i >= 0; i--) wrap = Fragment.from(conn[i].create(null, wrap));
			wrap = Fragment.from(before.copy(wrap));
			let tr = state.tr.step(new ReplaceAroundStep($cut.pos - 1, end, $cut.pos, end, new Slice(wrap, 1, 0), conn.length, true));
			let $joinAt = tr.doc.resolve(end + 2 * conn.length);
			if ($joinAt.nodeAfter && $joinAt.nodeAfter.type == before.type && canJoin(tr.doc, $joinAt.pos)) tr.join($joinAt.pos);
			dispatch(tr.scrollIntoView());
		}
		return true;
	}
	let selAfter = after.type.spec.isolating || dir > 0 && isolated ? null : Selection.findFrom($cut, 1);
	let range = selAfter && selAfter.$from.blockRange(selAfter.$to), target = range && liftTarget(range);
	if (target != null && target >= $cut.depth) {
		if (dispatch) dispatch(state.tr.lift(range, target).scrollIntoView());
		return true;
	}
	if (canDelAfter && textblockAt(after, "start", true) && textblockAt(before, "end")) {
		let at = before, wrap = [];
		for (;;) {
			wrap.push(at);
			if (at.isTextblock) break;
			at = at.lastChild;
		}
		let afterText = after, afterDepth = 1;
		for (; !afterText.isTextblock; afterText = afterText.firstChild) afterDepth++;
		if (at.canReplace(at.childCount, at.childCount, afterText.content)) {
			if (dispatch) {
				let end = Fragment.empty;
				for (let i = wrap.length - 1; i >= 0; i--) end = Fragment.from(wrap[i].copy(end));
				dispatch(state.tr.step(new ReplaceAroundStep($cut.pos - wrap.length, $cut.pos + after.nodeSize, $cut.pos + afterDepth, $cut.pos + after.nodeSize - afterDepth, new Slice(end, wrap.length, 0), 0, true)).scrollIntoView());
			}
			return true;
		}
	}
	return false;
}
function selectTextblockSide(side) {
	return function(state, dispatch) {
		let sel = state.selection, $pos = side < 0 ? sel.$from : sel.$to;
		let depth = $pos.depth;
		while ($pos.node(depth).isInline) {
			if (!depth) return false;
			depth--;
		}
		if (!$pos.node(depth).isTextblock) return false;
		if (dispatch) dispatch(state.tr.setSelection(TextSelection.create(state.doc, side < 0 ? $pos.start(depth) : $pos.end(depth))));
		return true;
	};
}
/**
Moves the cursor to the start of current text block.
*/
var selectTextblockStart$1 = selectTextblockSide(-1);
/**
Moves the cursor to the end of current text block.
*/
var selectTextblockEnd$1 = selectTextblockSide(1);
/**
Wrap the selection in a node of the given type with the given
attributes.
*/
function wrapIn$1(nodeType, attrs = null) {
	return function(state, dispatch) {
		let { $from, $to } = state.selection;
		let range = $from.blockRange($to), wrapping = range && findWrapping(range, nodeType, attrs);
		if (!wrapping) return false;
		if (dispatch) dispatch(state.tr.wrap(range, wrapping).scrollIntoView());
		return true;
	};
}
/**
Returns a command that tries to set the selected textblocks to the
given node type with the given attributes.
*/
function setBlockType(nodeType, attrs = null) {
	return function(state, dispatch) {
		let applicable = false;
		for (let i = 0; i < state.selection.ranges.length && !applicable; i++) {
			let { $from: { pos: from }, $to: { pos: to } } = state.selection.ranges[i];
			state.doc.nodesBetween(from, to, (node, pos) => {
				if (applicable) return false;
				if (!node.isTextblock || node.hasMarkup(nodeType, attrs)) return;
				if (node.type == nodeType) applicable = true;
				else {
					let $pos = state.doc.resolve(pos), index = $pos.index();
					applicable = $pos.parent.canReplaceWith(index, index + 1, nodeType);
				}
			});
		}
		if (!applicable) return false;
		if (dispatch) {
			let tr = state.tr;
			for (let i = 0; i < state.selection.ranges.length; i++) {
				let { $from: { pos: from }, $to: { pos: to } } = state.selection.ranges[i];
				tr.setBlockType(from, to, nodeType, attrs);
			}
			dispatch(tr.scrollIntoView());
		}
		return true;
	};
}
/**
Combine a number of command functions into a single function (which
calls them one by one until one returns true).
*/
function chainCommands(...commands) {
	return function(state, dispatch, view) {
		for (let i = 0; i < commands.length; i++) if (commands[i](state, dispatch, view)) return true;
		return false;
	};
}
var backspace = chainCommands(deleteSelection$1, joinBackward$1, selectNodeBackward$1);
var del = chainCommands(deleteSelection$1, joinForward$1, selectNodeForward$1);
/**
A basic keymap containing bindings not specific to any schema.
Binds the following keys (when multiple commands are listed, they
are chained with [`chainCommands`](https://prosemirror.net/docs/ref/#commands.chainCommands)):

* **Enter** to `newlineInCode`, `createParagraphNear`, `liftEmptyBlock`, `splitBlock`
* **Mod-Enter** to `exitCode`
* **Backspace** and **Mod-Backspace** to `deleteSelection`, `joinBackward`, `selectNodeBackward`
* **Delete** and **Mod-Delete** to `deleteSelection`, `joinForward`, `selectNodeForward`
* **Mod-Delete** to `deleteSelection`, `joinForward`, `selectNodeForward`
* **Mod-a** to `selectAll`
*/
var pcBaseKeymap = {
	"Enter": chainCommands(newlineInCode$1, createParagraphNear$1, liftEmptyBlock$1, splitBlock$1),
	"Mod-Enter": exitCode$1,
	"Backspace": backspace,
	"Mod-Backspace": backspace,
	"Shift-Backspace": backspace,
	"Delete": del,
	"Mod-Delete": del,
	"Mod-a": selectAll$1
};
/**
A copy of `pcBaseKeymap` that also binds **Ctrl-h** like Backspace,
**Ctrl-d** like Delete, **Alt-Backspace** like Ctrl-Backspace, and
**Ctrl-Alt-Backspace**, **Alt-Delete**, and **Alt-d** like
Ctrl-Delete.
*/
var macBaseKeymap = {
	"Ctrl-h": pcBaseKeymap["Backspace"],
	"Alt-Backspace": pcBaseKeymap["Mod-Backspace"],
	"Ctrl-d": pcBaseKeymap["Delete"],
	"Ctrl-Alt-Backspace": pcBaseKeymap["Mod-Delete"],
	"Alt-Delete": pcBaseKeymap["Mod-Delete"],
	"Alt-d": pcBaseKeymap["Mod-Delete"],
	"Ctrl-a": selectTextblockStart$1,
	"Ctrl-e": selectTextblockEnd$1
};
for (let key in pcBaseKeymap) macBaseKeymap[key] = pcBaseKeymap[key];
typeof navigator != "undefined" ? /Mac|iP(hone|[oa]d)/.test(navigator.platform) : typeof os != "undefined" && os.platform && os.platform();
//#endregion
//#region ../../node_modules/prosemirror-schema-list/dist/index.js
/**
Returns a command function that wraps the selection in a list with
the given type an attributes. If `dispatch` is null, only return a
value to indicate whether this is possible, but don't actually
perform the change.
*/
function wrapInList$1(listType, attrs = null) {
	return function(state, dispatch) {
		let { $from, $to } = state.selection;
		let range = $from.blockRange($to);
		if (!range) return false;
		let tr = dispatch ? state.tr : null;
		if (!wrapRangeInList(tr, range, listType, attrs)) return false;
		if (dispatch) dispatch(tr.scrollIntoView());
		return true;
	};
}
/**
Try to wrap the given node range in a list of the given type.
Return `true` when this is possible, `false` otherwise. When `tr`
is non-null, the wrapping is added to that transaction. When it is
`null`, the function only queries whether the wrapping is
possible.
*/
function wrapRangeInList(tr, range, listType, attrs = null) {
	let doJoin = false, outerRange = range, doc = range.$from.doc;
	if (range.depth >= 2 && range.$from.node(range.depth - 1).type.compatibleContent(listType) && range.startIndex == 0) {
		if (range.$from.index(range.depth - 1) == 0) return false;
		let $insert = doc.resolve(range.start - 2);
		outerRange = new NodeRange($insert, $insert, range.depth);
		if (range.endIndex < range.parent.childCount) range = new NodeRange(range.$from, doc.resolve(range.$to.end(range.depth)), range.depth);
		doJoin = true;
	}
	let wrap = findWrapping(outerRange, listType, attrs, range);
	if (!wrap) return false;
	if (tr) doWrapInList(tr, range, wrap, doJoin, listType);
	return true;
}
function doWrapInList(tr, range, wrappers, joinBefore, listType) {
	let content = Fragment.empty;
	for (let i = wrappers.length - 1; i >= 0; i--) content = Fragment.from(wrappers[i].type.create(wrappers[i].attrs, content));
	tr.step(new ReplaceAroundStep(range.start - (joinBefore ? 2 : 0), range.end, range.start, range.end, new Slice(content, 0, 0), wrappers.length, true));
	let found = 0;
	for (let i = 0; i < wrappers.length; i++) if (wrappers[i].type == listType) found = i + 1;
	let splitDepth = wrappers.length - found;
	let splitPos = range.start + wrappers.length - (joinBefore ? 2 : 0), parent = range.parent;
	for (let i = range.startIndex, e = range.endIndex, first = true; i < e; i++, first = false) {
		if (!first && canSplit(tr.doc, splitPos, splitDepth)) {
			tr.split(splitPos, splitDepth);
			splitPos += 2 * splitDepth;
		}
		splitPos += parent.child(i).nodeSize;
	}
	return tr;
}
/**
Create a command to lift the list item around the selection up into
a wrapping list.
*/
function liftListItem$1(itemType) {
	return function(state, dispatch) {
		let { $from, $to } = state.selection;
		let range = $from.blockRange($to, (node) => node.childCount > 0 && node.firstChild.type == itemType);
		if (!range) return false;
		if (!dispatch) return true;
		if ($from.node(range.depth - 1).type == itemType) return liftToOuterList(state, dispatch, itemType, range);
		else return liftOutOfList(state, dispatch, range);
	};
}
function liftToOuterList(state, dispatch, itemType, range) {
	let tr = state.tr, end = range.end, endOfList = range.$to.end(range.depth);
	if (end < endOfList) {
		tr.step(new ReplaceAroundStep(end - 1, endOfList, end, endOfList, new Slice(Fragment.from(itemType.create(null, range.parent.copy())), 1, 0), 1, true));
		range = new NodeRange(tr.doc.resolve(range.$from.pos), tr.doc.resolve(endOfList), range.depth);
	}
	const target = liftTarget(range);
	if (target == null) return false;
	tr.lift(range, target);
	let $after = tr.doc.resolve(tr.mapping.map(end, -1) - 1);
	if (canJoin(tr.doc, $after.pos) && $after.nodeBefore.type == $after.nodeAfter.type) tr.join($after.pos);
	dispatch(tr.scrollIntoView());
	return true;
}
function liftOutOfList(state, dispatch, range) {
	let tr = state.tr, list = range.parent;
	for (let pos = range.end, i = range.endIndex - 1, e = range.startIndex; i > e; i--) {
		pos -= list.child(i).nodeSize;
		tr.delete(pos - 1, pos + 1);
	}
	let $start = tr.doc.resolve(range.start), item = $start.nodeAfter;
	if (tr.mapping.map(range.end) != range.start + $start.nodeAfter.nodeSize) return false;
	let atStart = range.startIndex == 0, atEnd = range.endIndex == list.childCount;
	let parent = $start.node(-1), indexBefore = $start.index(-1);
	if (!parent.canReplace(indexBefore + (atStart ? 0 : 1), indexBefore + 1, item.content.append(atEnd ? Fragment.empty : Fragment.from(list)))) return false;
	let start = $start.pos, end = start + item.nodeSize;
	tr.step(new ReplaceAroundStep(start - (atStart ? 1 : 0), end + (atEnd ? 1 : 0), start + 1, end - 1, new Slice((atStart ? Fragment.empty : Fragment.from(list.copy(Fragment.empty))).append(atEnd ? Fragment.empty : Fragment.from(list.copy(Fragment.empty))), atStart ? 0 : 1, atEnd ? 0 : 1), atStart ? 0 : 1));
	dispatch(tr.scrollIntoView());
	return true;
}
/**
Create a command to sink the list item around the selection down
into an inner list.
*/
function sinkListItem$1(itemType) {
	return function(state, dispatch) {
		let { $from, $to } = state.selection;
		let range = $from.blockRange($to, (node) => node.childCount > 0 && node.firstChild.type == itemType);
		if (!range) return false;
		let startIndex = range.startIndex;
		if (startIndex == 0) return false;
		let parent = range.parent, nodeBefore = parent.child(startIndex - 1);
		if (nodeBefore.type != itemType) return false;
		if (dispatch) {
			let nestedBefore = nodeBefore.lastChild && nodeBefore.lastChild.type == parent.type;
			let inner = Fragment.from(nestedBefore ? itemType.create() : null);
			let slice = new Slice(Fragment.from(itemType.create(null, Fragment.from(parent.type.create(null, inner)))), nestedBefore ? 3 : 1, 0);
			let before = range.start, after = range.end;
			dispatch(state.tr.step(new ReplaceAroundStep(before - (nestedBefore ? 3 : 1), after, before, after, slice, 1, true)).scrollIntoView());
		}
		return true;
	};
}
//#endregion
//#region ../../node_modules/@tiptap/core/dist/index.js
/**
* Takes a Transaction & Editor State and turns it into a chainable state object
* @param config The transaction and state to create the chainable state from
* @returns A chainable Editor state object
*/
function createChainableState(config) {
	const { state, transaction } = config;
	let { selection } = transaction;
	let { doc } = transaction;
	let { storedMarks } = transaction;
	return {
		...state,
		apply: state.apply.bind(state),
		applyTransaction: state.applyTransaction.bind(state),
		plugins: state.plugins,
		schema: state.schema,
		reconfigure: state.reconfigure.bind(state),
		toJSON: state.toJSON.bind(state),
		get storedMarks() {
			return storedMarks;
		},
		get selection() {
			return selection;
		},
		get doc() {
			return doc;
		},
		get tr() {
			selection = transaction.selection;
			doc = transaction.doc;
			storedMarks = transaction.storedMarks;
			return transaction;
		}
	};
}
var CommandManager = class {
	constructor(props) {
		this.editor = props.editor;
		this.rawCommands = this.editor.extensionManager.commands;
		this.customState = props.state;
	}
	get hasCustomState() {
		return !!this.customState;
	}
	get state() {
		return this.customState || this.editor.state;
	}
	get commands() {
		const { rawCommands, editor, state } = this;
		const { view } = editor;
		const { tr } = state;
		const props = this.buildProps(tr);
		return Object.fromEntries(Object.entries(rawCommands).map(([name, command]) => {
			const method = (...args) => {
				const callback = command(...args)(props);
				if (!tr.getMeta("preventDispatch") && !this.hasCustomState) view.dispatch(tr);
				return callback;
			};
			return [name, method];
		}));
	}
	get chain() {
		return () => this.createChain();
	}
	get can() {
		return () => this.createCan();
	}
	createChain(startTr, shouldDispatch = true) {
		const { rawCommands, editor, state } = this;
		const { view } = editor;
		const callbacks = [];
		const hasStartTransaction = !!startTr;
		const tr = startTr || state.tr;
		const run = () => {
			if (!hasStartTransaction && shouldDispatch && !tr.getMeta("preventDispatch") && !this.hasCustomState) view.dispatch(tr);
			return callbacks.every((callback) => callback === true);
		};
		const chain = {
			...Object.fromEntries(Object.entries(rawCommands).map(([name, command]) => {
				const chainedCommand = (...args) => {
					const props = this.buildProps(tr, shouldDispatch);
					const callback = command(...args)(props);
					callbacks.push(callback);
					return chain;
				};
				return [name, chainedCommand];
			})),
			run
		};
		return chain;
	}
	createCan(startTr) {
		const { rawCommands, state } = this;
		const dispatch = false;
		const tr = startTr || state.tr;
		const props = this.buildProps(tr, dispatch);
		return {
			...Object.fromEntries(Object.entries(rawCommands).map(([name, command]) => {
				return [name, (...args) => command(...args)({
					...props,
					dispatch: void 0
				})];
			})),
			chain: () => this.createChain(tr, dispatch)
		};
	}
	buildProps(tr, shouldDispatch = true) {
		const { rawCommands, editor, state } = this;
		const { view } = editor;
		const props = {
			tr,
			editor,
			view,
			state: createChainableState({
				state,
				transaction: tr
			}),
			dispatch: shouldDispatch ? () => void 0 : void 0,
			chain: () => this.createChain(tr, shouldDispatch),
			can: () => this.createCan(tr),
			get commands() {
				return Object.fromEntries(Object.entries(rawCommands).map(([name, command]) => {
					return [name, (...args) => command(...args)(props)];
				}));
			}
		};
		return props;
	}
};
var EventEmitter = class {
	constructor() {
		this.callbacks = {};
	}
	on(event, fn) {
		if (!this.callbacks[event]) this.callbacks[event] = [];
		this.callbacks[event].push(fn);
		return this;
	}
	emit(event, ...args) {
		const callbacks = this.callbacks[event];
		if (callbacks) callbacks.forEach((callback) => callback.apply(this, args));
		return this;
	}
	off(event, fn) {
		const callbacks = this.callbacks[event];
		if (callbacks) {
			if (fn) this.callbacks[event] = callbacks.filter((callback) => callback !== fn);
			else delete this.callbacks[event];
		}
		return this;
	}
	once(event, fn) {
		const onceFn = (...args) => {
			this.off(event, onceFn);
			fn.apply(this, args);
		};
		return this.on(event, onceFn);
	}
	removeAllListeners() {
		this.callbacks = {};
	}
};
/**
* Returns a field from an extension
* @param extension The Tiptap extension
* @param field The field, for example `renderHTML` or `priority`
* @param context The context object that should be passed as `this` into the function
* @returns The field value
*/
function getExtensionField(extension, field, context) {
	if (extension.config[field] === void 0 && extension.parent) return getExtensionField(extension.parent, field, context);
	if (typeof extension.config[field] === "function") return extension.config[field].bind({
		...context,
		parent: extension.parent ? getExtensionField(extension.parent, field, context) : null
	});
	return extension.config[field];
}
function splitExtensions(extensions) {
	return {
		baseExtensions: extensions.filter((extension) => extension.type === "extension"),
		nodeExtensions: extensions.filter((extension) => extension.type === "node"),
		markExtensions: extensions.filter((extension) => extension.type === "mark")
	};
}
/**
* Get a list of all extension attributes defined in `addAttribute` and `addGlobalAttribute`.
* @param extensions List of extensions
*/
function getAttributesFromExtensions(extensions) {
	const extensionAttributes = [];
	const { nodeExtensions, markExtensions } = splitExtensions(extensions);
	const nodeAndMarkExtensions = [...nodeExtensions, ...markExtensions];
	const defaultAttribute = {
		default: null,
		rendered: true,
		renderHTML: null,
		parseHTML: null,
		keepOnSplit: true,
		isRequired: false
	};
	extensions.forEach((extension) => {
		const addGlobalAttributes = getExtensionField(extension, "addGlobalAttributes", {
			name: extension.name,
			options: extension.options,
			storage: extension.storage,
			extensions: nodeAndMarkExtensions
		});
		if (!addGlobalAttributes) return;
		addGlobalAttributes().forEach((globalAttribute) => {
			globalAttribute.types.forEach((type) => {
				Object.entries(globalAttribute.attributes).forEach(([name, attribute]) => {
					extensionAttributes.push({
						type,
						name,
						attribute: {
							...defaultAttribute,
							...attribute
						}
					});
				});
			});
		});
	});
	nodeAndMarkExtensions.forEach((extension) => {
		const addAttributes = getExtensionField(extension, "addAttributes", {
			name: extension.name,
			options: extension.options,
			storage: extension.storage
		});
		if (!addAttributes) return;
		const attributes = addAttributes();
		Object.entries(attributes).forEach(([name, attribute]) => {
			const mergedAttr = {
				...defaultAttribute,
				...attribute
			};
			if (typeof (mergedAttr === null || mergedAttr === void 0 ? void 0 : mergedAttr.default) === "function") mergedAttr.default = mergedAttr.default();
			if ((mergedAttr === null || mergedAttr === void 0 ? void 0 : mergedAttr.isRequired) && (mergedAttr === null || mergedAttr === void 0 ? void 0 : mergedAttr.default) === void 0) delete mergedAttr.default;
			extensionAttributes.push({
				type: extension.name,
				name,
				attribute: mergedAttr
			});
		});
	});
	return extensionAttributes;
}
function getNodeType(nameOrType, schema) {
	if (typeof nameOrType === "string") {
		if (!schema.nodes[nameOrType]) throw Error(`There is no node type named '${nameOrType}'. Maybe you forgot to add the extension?`);
		return schema.nodes[nameOrType];
	}
	return nameOrType;
}
function mergeAttributes(...objects) {
	return objects.filter((item) => !!item).reduce((items, item) => {
		const mergedAttributes = { ...items };
		Object.entries(item).forEach(([key, value]) => {
			if (!mergedAttributes[key]) {
				mergedAttributes[key] = value;
				return;
			}
			if (key === "class") {
				const valueClasses = value ? String(value).split(" ") : [];
				const existingClasses = mergedAttributes[key] ? mergedAttributes[key].split(" ") : [];
				const insertClasses = valueClasses.filter((valueClass) => !existingClasses.includes(valueClass));
				mergedAttributes[key] = [...existingClasses, ...insertClasses].join(" ");
			} else if (key === "style") {
				const newStyles = value ? value.split(";").map((style) => style.trim()).filter(Boolean) : [];
				const existingStyles = mergedAttributes[key] ? mergedAttributes[key].split(";").map((style) => style.trim()).filter(Boolean) : [];
				const styleMap = /* @__PURE__ */ new Map();
				existingStyles.forEach((style) => {
					const [property, val] = style.split(":").map((part) => part.trim());
					styleMap.set(property, val);
				});
				newStyles.forEach((style) => {
					const [property, val] = style.split(":").map((part) => part.trim());
					styleMap.set(property, val);
				});
				mergedAttributes[key] = Array.from(styleMap.entries()).map(([property, val]) => `${property}: ${val}`).join("; ");
			} else mergedAttributes[key] = value;
		});
		return mergedAttributes;
	}, {});
}
function getRenderedAttributes(nodeOrMark, extensionAttributes) {
	return extensionAttributes.filter((attribute) => attribute.type === nodeOrMark.type.name).filter((item) => item.attribute.rendered).map((item) => {
		if (!item.attribute.renderHTML) return { [item.name]: nodeOrMark.attrs[item.name] };
		return item.attribute.renderHTML(nodeOrMark.attrs) || {};
	}).reduce((attributes, attribute) => mergeAttributes(attributes, attribute), {});
}
function isFunction(value) {
	return typeof value === "function";
}
/**
* Optionally calls `value` as a function.
* Otherwise it is returned directly.
* @param value Function or any value.
* @param context Optional context to bind to function.
* @param props Optional props to pass to function.
*/
function callOrReturn(value, context = void 0, ...props) {
	if (isFunction(value)) {
		if (context) return value.bind(context)(...props);
		return value(...props);
	}
	return value;
}
function isEmptyObject(value = {}) {
	return Object.keys(value).length === 0 && value.constructor === Object;
}
function fromString(value) {
	if (typeof value !== "string") return value;
	if (value.match(/^[+-]?(?:\d*\.)?\d+$/)) return Number(value);
	if (value === "true") return true;
	if (value === "false") return false;
	return value;
}
/**
* This function merges extension attributes into parserule attributes (`attrs` or `getAttrs`).
* Cancels when `getAttrs` returned `false`.
* @param parseRule ProseMirror ParseRule
* @param extensionAttributes List of attributes to inject
*/
function injectExtensionAttributesToParseRule(parseRule, extensionAttributes) {
	if ("style" in parseRule) return parseRule;
	return {
		...parseRule,
		getAttrs: (node) => {
			const oldAttributes = parseRule.getAttrs ? parseRule.getAttrs(node) : parseRule.attrs;
			if (oldAttributes === false) return false;
			const newAttributes = extensionAttributes.reduce((items, item) => {
				const value = item.attribute.parseHTML ? item.attribute.parseHTML(node) : fromString(node.getAttribute(item.name));
				if (value === null || value === void 0) return items;
				return {
					...items,
					[item.name]: value
				};
			}, {});
			return {
				...oldAttributes,
				...newAttributes
			};
		}
	};
}
function cleanUpSchemaItem(data) {
	return Object.fromEntries(Object.entries(data).filter(([key, value]) => {
		if (key === "attrs" && isEmptyObject(value)) return false;
		return value !== null && value !== void 0;
	}));
}
/**
* Creates a new Prosemirror schema based on the given extensions.
* @param extensions An array of Tiptap extensions
* @param editor The editor instance
* @returns A Prosemirror schema
*/
function getSchemaByResolvedExtensions(extensions, editor) {
	var _a;
	const allAttributes = getAttributesFromExtensions(extensions);
	const { nodeExtensions, markExtensions } = splitExtensions(extensions);
	const topNode = (_a = nodeExtensions.find((extension) => getExtensionField(extension, "topNode"))) === null || _a === void 0 ? void 0 : _a.name;
	const nodes = Object.fromEntries(nodeExtensions.map((extension) => {
		const extensionAttributes = allAttributes.filter((attribute) => attribute.type === extension.name);
		const context = {
			name: extension.name,
			options: extension.options,
			storage: extension.storage,
			editor
		};
		const schema = cleanUpSchemaItem({
			...extensions.reduce((fields, e) => {
				const extendNodeSchema = getExtensionField(e, "extendNodeSchema", context);
				return {
					...fields,
					...extendNodeSchema ? extendNodeSchema(extension) : {}
				};
			}, {}),
			content: callOrReturn(getExtensionField(extension, "content", context)),
			marks: callOrReturn(getExtensionField(extension, "marks", context)),
			group: callOrReturn(getExtensionField(extension, "group", context)),
			inline: callOrReturn(getExtensionField(extension, "inline", context)),
			atom: callOrReturn(getExtensionField(extension, "atom", context)),
			selectable: callOrReturn(getExtensionField(extension, "selectable", context)),
			draggable: callOrReturn(getExtensionField(extension, "draggable", context)),
			code: callOrReturn(getExtensionField(extension, "code", context)),
			whitespace: callOrReturn(getExtensionField(extension, "whitespace", context)),
			linebreakReplacement: callOrReturn(getExtensionField(extension, "linebreakReplacement", context)),
			defining: callOrReturn(getExtensionField(extension, "defining", context)),
			isolating: callOrReturn(getExtensionField(extension, "isolating", context)),
			attrs: Object.fromEntries(extensionAttributes.map((extensionAttribute) => {
				var _a;
				return [extensionAttribute.name, { default: (_a = extensionAttribute === null || extensionAttribute === void 0 ? void 0 : extensionAttribute.attribute) === null || _a === void 0 ? void 0 : _a.default }];
			}))
		});
		const parseHTML = callOrReturn(getExtensionField(extension, "parseHTML", context));
		if (parseHTML) schema.parseDOM = parseHTML.map((parseRule) => injectExtensionAttributesToParseRule(parseRule, extensionAttributes));
		const renderHTML = getExtensionField(extension, "renderHTML", context);
		if (renderHTML) schema.toDOM = (node) => renderHTML({
			node,
			HTMLAttributes: getRenderedAttributes(node, extensionAttributes)
		});
		const renderText = getExtensionField(extension, "renderText", context);
		if (renderText) schema.toText = renderText;
		return [extension.name, schema];
	}));
	const marks = Object.fromEntries(markExtensions.map((extension) => {
		const extensionAttributes = allAttributes.filter((attribute) => attribute.type === extension.name);
		const context = {
			name: extension.name,
			options: extension.options,
			storage: extension.storage,
			editor
		};
		const schema = cleanUpSchemaItem({
			...extensions.reduce((fields, e) => {
				const extendMarkSchema = getExtensionField(e, "extendMarkSchema", context);
				return {
					...fields,
					...extendMarkSchema ? extendMarkSchema(extension) : {}
				};
			}, {}),
			inclusive: callOrReturn(getExtensionField(extension, "inclusive", context)),
			excludes: callOrReturn(getExtensionField(extension, "excludes", context)),
			group: callOrReturn(getExtensionField(extension, "group", context)),
			spanning: callOrReturn(getExtensionField(extension, "spanning", context)),
			code: callOrReturn(getExtensionField(extension, "code", context)),
			attrs: Object.fromEntries(extensionAttributes.map((extensionAttribute) => {
				var _a;
				return [extensionAttribute.name, { default: (_a = extensionAttribute === null || extensionAttribute === void 0 ? void 0 : extensionAttribute.attribute) === null || _a === void 0 ? void 0 : _a.default }];
			}))
		});
		const parseHTML = callOrReturn(getExtensionField(extension, "parseHTML", context));
		if (parseHTML) schema.parseDOM = parseHTML.map((parseRule) => injectExtensionAttributesToParseRule(parseRule, extensionAttributes));
		const renderHTML = getExtensionField(extension, "renderHTML", context);
		if (renderHTML) schema.toDOM = (mark) => renderHTML({
			mark,
			HTMLAttributes: getRenderedAttributes(mark, extensionAttributes)
		});
		return [extension.name, schema];
	}));
	return new Schema({
		topNode,
		nodes,
		marks
	});
}
/**
* Tries to get a node or mark type by its name.
* @param name The name of the node or mark type
* @param schema The Prosemiror schema to search in
* @returns The node or mark type, or null if it doesn't exist
*/
function getSchemaTypeByName(name, schema) {
	return schema.nodes[name] || schema.marks[name] || null;
}
function isExtensionRulesEnabled(extension, enabled) {
	if (Array.isArray(enabled)) return enabled.some((enabledExtension) => {
		return (typeof enabledExtension === "string" ? enabledExtension : enabledExtension.name) === extension.name;
	});
	return enabled;
}
function getHTMLFromFragment(fragment, schema) {
	const documentFragment = DOMSerializer.fromSchema(schema).serializeFragment(fragment);
	const container = document.implementation.createHTMLDocument().createElement("div");
	container.appendChild(documentFragment);
	return container.innerHTML;
}
/**
* Returns the text content of a resolved prosemirror position
* @param $from The resolved position to get the text content from
* @param maxMatch The maximum number of characters to match
* @returns The text content
*/
var getTextContentFromNodes = ($from, maxMatch = 500) => {
	let textBefore = "";
	const sliceEndPos = $from.parentOffset;
	$from.parent.nodesBetween(Math.max(0, sliceEndPos - maxMatch), sliceEndPos, (node, pos, parent, index) => {
		var _a, _b;
		const chunk = ((_b = (_a = node.type.spec).toText) === null || _b === void 0 ? void 0 : _b.call(_a, {
			node,
			pos,
			parent,
			index
		})) || node.textContent || "%leaf%";
		textBefore += node.isAtom && !node.isText ? chunk : chunk.slice(0, Math.max(0, sliceEndPos - pos));
	});
	return textBefore;
};
function isRegExp(value) {
	return Object.prototype.toString.call(value) === "[object RegExp]";
}
var InputRule = class {
	constructor(config) {
		this.find = config.find;
		this.handler = config.handler;
	}
};
var inputRuleMatcherHandler = (text, find) => {
	if (isRegExp(find)) return find.exec(text);
	const inputRuleMatch = find(text);
	if (!inputRuleMatch) return null;
	const result = [inputRuleMatch.text];
	result.index = inputRuleMatch.index;
	result.input = text;
	result.data = inputRuleMatch.data;
	if (inputRuleMatch.replaceWith) {
		if (!inputRuleMatch.text.includes(inputRuleMatch.replaceWith)) console.warn("[tiptap warn]: \"inputRuleMatch.replaceWith\" must be part of \"inputRuleMatch.text\".");
		result.push(inputRuleMatch.replaceWith);
	}
	return result;
};
function run$1(config) {
	var _a;
	const { editor, from, to, text, rules, plugin } = config;
	const { view } = editor;
	if (view.composing) return false;
	const $from = view.state.doc.resolve(from);
	if ($from.parent.type.spec.code || !!((_a = $from.nodeBefore || $from.nodeAfter) === null || _a === void 0 ? void 0 : _a.marks.find((mark) => mark.type.spec.code))) return false;
	let matched = false;
	const textBefore = getTextContentFromNodes($from) + text;
	rules.forEach((rule) => {
		if (matched) return;
		const match = inputRuleMatcherHandler(textBefore, rule.find);
		if (!match) return;
		const tr = view.state.tr;
		const state = createChainableState({
			state: view.state,
			transaction: tr
		});
		const range = {
			from: from - (match[0].length - text.length),
			to
		};
		const { commands, chain, can } = new CommandManager({
			editor,
			state
		});
		if (rule.handler({
			state,
			range,
			match,
			commands,
			chain,
			can
		}) === null || !tr.steps.length) return;
		tr.setMeta(plugin, {
			transform: tr,
			from,
			to,
			text
		});
		view.dispatch(tr);
		matched = true;
	});
	return matched;
}
/**
* Create an input rules plugin. When enabled, it will cause text
* input that matches any of the given rules to trigger the rule’s
* action.
*/
function inputRulesPlugin(props) {
	const { editor, rules } = props;
	const plugin = new Plugin({
		state: {
			init() {
				return null;
			},
			apply(tr, prev, state) {
				const stored = tr.getMeta(plugin);
				if (stored) return stored;
				const simulatedInputMeta = tr.getMeta("applyInputRules");
				if (!!simulatedInputMeta) setTimeout(() => {
					let { text } = simulatedInputMeta;
					if (typeof text === "string") text = text;
					else text = getHTMLFromFragment(Fragment.from(text), state.schema);
					const { from } = simulatedInputMeta;
					const to = from + text.length;
					run$1({
						editor,
						from,
						to,
						text,
						rules,
						plugin
					});
				});
				return tr.selectionSet || tr.docChanged ? null : prev;
			}
		},
		props: {
			handleTextInput(view, from, to, text) {
				return run$1({
					editor,
					from,
					to,
					text,
					rules,
					plugin
				});
			},
			handleDOMEvents: { compositionend: (view) => {
				setTimeout(() => {
					const { $cursor } = view.state.selection;
					if ($cursor) run$1({
						editor,
						from: $cursor.pos,
						to: $cursor.pos,
						text: "",
						rules,
						plugin
					});
				});
				return false;
			} },
			handleKeyDown(view, event) {
				if (event.key !== "Enter") return false;
				const { $cursor } = view.state.selection;
				if ($cursor) return run$1({
					editor,
					from: $cursor.pos,
					to: $cursor.pos,
					text: "\n",
					rules,
					plugin
				});
				return false;
			}
		},
		isInputRules: true
	});
	return plugin;
}
function getType(value) {
	return Object.prototype.toString.call(value).slice(8, -1);
}
function isPlainObject(value) {
	if (getType(value) !== "Object") return false;
	return value.constructor === Object && Object.getPrototypeOf(value) === Object.prototype;
}
function mergeDeep(target, source) {
	const output = { ...target };
	if (isPlainObject(target) && isPlainObject(source)) Object.keys(source).forEach((key) => {
		if (isPlainObject(source[key]) && isPlainObject(target[key])) output[key] = mergeDeep(target[key], source[key]);
		else output[key] = source[key];
	});
	return output;
}
/**
* The Mark class is used to create custom mark extensions.
* @see https://tiptap.dev/api/extensions#create-a-new-extension
*/
var Mark = class Mark {
	constructor(config = {}) {
		this.type = "mark";
		this.name = "mark";
		this.parent = null;
		this.child = null;
		this.config = {
			name: this.name,
			defaultOptions: {}
		};
		this.config = {
			...this.config,
			...config
		};
		this.name = this.config.name;
		if (config.defaultOptions && Object.keys(config.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${this.name}".`);
		this.options = this.config.defaultOptions;
		if (this.config.addOptions) this.options = callOrReturn(getExtensionField(this, "addOptions", { name: this.name }));
		this.storage = callOrReturn(getExtensionField(this, "addStorage", {
			name: this.name,
			options: this.options
		})) || {};
	}
	static create(config = {}) {
		return new Mark(config);
	}
	configure(options = {}) {
		const extension = this.extend({
			...this.config,
			addOptions: () => {
				return mergeDeep(this.options, options);
			}
		});
		extension.name = this.name;
		extension.parent = this.parent;
		return extension;
	}
	extend(extendedConfig = {}) {
		const extension = new Mark(extendedConfig);
		extension.parent = this;
		this.child = extension;
		extension.name = extendedConfig.name ? extendedConfig.name : extension.parent.name;
		if (extendedConfig.defaultOptions && Object.keys(extendedConfig.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${extension.name}".`);
		extension.options = callOrReturn(getExtensionField(extension, "addOptions", { name: extension.name }));
		extension.storage = callOrReturn(getExtensionField(extension, "addStorage", {
			name: extension.name,
			options: extension.options
		}));
		return extension;
	}
	static handleExit({ editor, mark }) {
		const { tr } = editor.state;
		const currentPos = editor.state.selection.$from;
		if (currentPos.pos === currentPos.end()) {
			const currentMarks = currentPos.marks();
			if (!!!currentMarks.find((m) => (m === null || m === void 0 ? void 0 : m.type.name) === mark.name)) return false;
			const removeMark = currentMarks.find((m) => (m === null || m === void 0 ? void 0 : m.type.name) === mark.name);
			if (removeMark) tr.removeStoredMark(removeMark);
			tr.insertText(" ", currentPos.pos);
			editor.view.dispatch(tr);
			return true;
		}
		return false;
	}
};
function isNumber(value) {
	return typeof value === "number";
}
/**
* Paste rules are used to react to pasted content.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#paste-rules
*/
var PasteRule = class {
	constructor(config) {
		this.find = config.find;
		this.handler = config.handler;
	}
};
var pasteRuleMatcherHandler = (text, find, event) => {
	if (isRegExp(find)) return [...text.matchAll(find)];
	const matches = find(text, event);
	if (!matches) return [];
	return matches.map((pasteRuleMatch) => {
		const result = [pasteRuleMatch.text];
		result.index = pasteRuleMatch.index;
		result.input = text;
		result.data = pasteRuleMatch.data;
		if (pasteRuleMatch.replaceWith) {
			if (!pasteRuleMatch.text.includes(pasteRuleMatch.replaceWith)) console.warn("[tiptap warn]: \"pasteRuleMatch.replaceWith\" must be part of \"pasteRuleMatch.text\".");
			result.push(pasteRuleMatch.replaceWith);
		}
		return result;
	});
};
function run(config) {
	const { editor, state, from, to, rule, pasteEvent, dropEvent } = config;
	const { commands, chain, can } = new CommandManager({
		editor,
		state
	});
	const handlers = [];
	state.doc.nodesBetween(from, to, (node, pos) => {
		if (!node.isTextblock || node.type.spec.code) return;
		const resolvedFrom = Math.max(from, pos);
		const resolvedTo = Math.min(to, pos + node.content.size);
		pasteRuleMatcherHandler(node.textBetween(resolvedFrom - pos, resolvedTo - pos, void 0, "￼"), rule.find, pasteEvent).forEach((match) => {
			if (match.index === void 0) return;
			const start = resolvedFrom + match.index + 1;
			const end = start + match[0].length;
			const range = {
				from: state.tr.mapping.map(start),
				to: state.tr.mapping.map(end)
			};
			const handler = rule.handler({
				state,
				range,
				match,
				commands,
				chain,
				can,
				pasteEvent,
				dropEvent
			});
			handlers.push(handler);
		});
	});
	return handlers.every((handler) => handler !== null);
}
var tiptapDragFromOtherEditor = null;
var createClipboardPasteEvent = (text) => {
	var _a;
	const event = new ClipboardEvent("paste", { clipboardData: new DataTransfer() });
	(_a = event.clipboardData) === null || _a === void 0 || _a.setData("text/html", text);
	return event;
};
/**
* Create an paste rules plugin. When enabled, it will cause pasted
* text that matches any of the given rules to trigger the rule’s
* action.
*/
function pasteRulesPlugin(props) {
	const { editor, rules } = props;
	let dragSourceElement = null;
	let isPastedFromProseMirror = false;
	let isDroppedFromProseMirror = false;
	let pasteEvent = typeof ClipboardEvent !== "undefined" ? new ClipboardEvent("paste") : null;
	let dropEvent;
	try {
		dropEvent = typeof DragEvent !== "undefined" ? new DragEvent("drop") : null;
	} catch {
		dropEvent = null;
	}
	const processEvent = ({ state, from, to, rule, pasteEvt }) => {
		const tr = state.tr;
		const chainableState = createChainableState({
			state,
			transaction: tr
		});
		if (!run({
			editor,
			state: chainableState,
			from: Math.max(from - 1, 0),
			to: to.b - 1,
			rule,
			pasteEvent: pasteEvt,
			dropEvent
		}) || !tr.steps.length) return;
		try {
			dropEvent = typeof DragEvent !== "undefined" ? new DragEvent("drop") : null;
		} catch {
			dropEvent = null;
		}
		pasteEvent = typeof ClipboardEvent !== "undefined" ? new ClipboardEvent("paste") : null;
		return tr;
	};
	return rules.map((rule) => {
		return new Plugin({
			view(view) {
				const handleDragstart = (event) => {
					var _a;
					dragSourceElement = ((_a = view.dom.parentElement) === null || _a === void 0 ? void 0 : _a.contains(event.target)) ? view.dom.parentElement : null;
					if (dragSourceElement) tiptapDragFromOtherEditor = editor;
				};
				const handleDragend = () => {
					if (tiptapDragFromOtherEditor) tiptapDragFromOtherEditor = null;
				};
				window.addEventListener("dragstart", handleDragstart);
				window.addEventListener("dragend", handleDragend);
				return { destroy() {
					window.removeEventListener("dragstart", handleDragstart);
					window.removeEventListener("dragend", handleDragend);
				} };
			},
			props: { handleDOMEvents: {
				drop: (view, event) => {
					isDroppedFromProseMirror = dragSourceElement === view.dom.parentElement;
					dropEvent = event;
					if (!isDroppedFromProseMirror) {
						const dragFromOtherEditor = tiptapDragFromOtherEditor;
						if (dragFromOtherEditor === null || dragFromOtherEditor === void 0 ? void 0 : dragFromOtherEditor.isEditable) setTimeout(() => {
							const selection = dragFromOtherEditor.state.selection;
							if (selection) dragFromOtherEditor.commands.deleteRange({
								from: selection.from,
								to: selection.to
							});
						}, 10);
					}
					return false;
				},
				paste: (_view, event) => {
					var _a;
					const html = (_a = event.clipboardData) === null || _a === void 0 ? void 0 : _a.getData("text/html");
					pasteEvent = event;
					isPastedFromProseMirror = !!(html === null || html === void 0 ? void 0 : html.includes("data-pm-slice"));
					return false;
				}
			} },
			appendTransaction: (transactions, oldState, state) => {
				const transaction = transactions[0];
				const isPaste = transaction.getMeta("uiEvent") === "paste" && !isPastedFromProseMirror;
				const isDrop = transaction.getMeta("uiEvent") === "drop" && !isDroppedFromProseMirror;
				const simulatedPasteMeta = transaction.getMeta("applyPasteRules");
				const isSimulatedPaste = !!simulatedPasteMeta;
				if (!isPaste && !isDrop && !isSimulatedPaste) return;
				if (isSimulatedPaste) {
					let { text } = simulatedPasteMeta;
					if (typeof text === "string") text = text;
					else text = getHTMLFromFragment(Fragment.from(text), state.schema);
					const { from } = simulatedPasteMeta;
					const to = from + text.length;
					const pasteEvt = createClipboardPasteEvent(text);
					return processEvent({
						rule,
						state,
						from,
						to: { b: to },
						pasteEvt
					});
				}
				const from = oldState.doc.content.findDiffStart(state.doc.content);
				const to = oldState.doc.content.findDiffEnd(state.doc.content);
				if (!isNumber(from) || !to || from === to.b) return;
				return processEvent({
					rule,
					state,
					from,
					to,
					pasteEvt: pasteEvent
				});
			}
		});
	});
}
function findDuplicates(items) {
	const filtered = items.filter((el, index) => items.indexOf(el) !== index);
	return Array.from(new Set(filtered));
}
var ExtensionManager = class ExtensionManager {
	constructor(extensions, editor) {
		this.splittableMarks = [];
		this.editor = editor;
		this.extensions = ExtensionManager.resolve(extensions);
		this.schema = getSchemaByResolvedExtensions(this.extensions, editor);
		this.setupExtensions();
	}
	/**
	* Returns a flattened and sorted extension list while
	* also checking for duplicated extensions and warns the user.
	* @param extensions An array of Tiptap extensions
	* @returns An flattened and sorted array of Tiptap extensions
	*/
	static resolve(extensions) {
		const resolvedExtensions = ExtensionManager.sort(ExtensionManager.flatten(extensions));
		const duplicatedNames = findDuplicates(resolvedExtensions.map((extension) => extension.name));
		if (duplicatedNames.length) console.warn(`[tiptap warn]: Duplicate extension names found: [${duplicatedNames.map((item) => `'${item}'`).join(", ")}]. This can lead to issues.`);
		return resolvedExtensions;
	}
	/**
	* Create a flattened array of extensions by traversing the `addExtensions` field.
	* @param extensions An array of Tiptap extensions
	* @returns A flattened array of Tiptap extensions
	*/
	static flatten(extensions) {
		return extensions.map((extension) => {
			const addExtensions = getExtensionField(extension, "addExtensions", {
				name: extension.name,
				options: extension.options,
				storage: extension.storage
			});
			if (addExtensions) return [extension, ...this.flatten(addExtensions())];
			return extension;
		}).flat(10);
	}
	/**
	* Sort extensions by priority.
	* @param extensions An array of Tiptap extensions
	* @returns A sorted array of Tiptap extensions by priority
	*/
	static sort(extensions) {
		const defaultPriority = 100;
		return extensions.sort((a, b) => {
			const priorityA = getExtensionField(a, "priority") || defaultPriority;
			const priorityB = getExtensionField(b, "priority") || defaultPriority;
			if (priorityA > priorityB) return -1;
			if (priorityA < priorityB) return 1;
			return 0;
		});
	}
	/**
	* Get all commands from the extensions.
	* @returns An object with all commands where the key is the command name and the value is the command function
	*/
	get commands() {
		return this.extensions.reduce((commands, extension) => {
			const addCommands = getExtensionField(extension, "addCommands", {
				name: extension.name,
				options: extension.options,
				storage: extension.storage,
				editor: this.editor,
				type: getSchemaTypeByName(extension.name, this.schema)
			});
			if (!addCommands) return commands;
			return {
				...commands,
				...addCommands()
			};
		}, {});
	}
	/**
	* Get all registered Prosemirror plugins from the extensions.
	* @returns An array of Prosemirror plugins
	*/
	get plugins() {
		const { editor } = this;
		const extensions = ExtensionManager.sort([...this.extensions].reverse());
		const inputRules = [];
		const pasteRules = [];
		const allPlugins = extensions.map((extension) => {
			const context = {
				name: extension.name,
				options: extension.options,
				storage: extension.storage,
				editor,
				type: getSchemaTypeByName(extension.name, this.schema)
			};
			const plugins = [];
			const addKeyboardShortcuts = getExtensionField(extension, "addKeyboardShortcuts", context);
			let defaultBindings = {};
			if (extension.type === "mark" && getExtensionField(extension, "exitable", context)) defaultBindings.ArrowRight = () => Mark.handleExit({
				editor,
				mark: extension
			});
			if (addKeyboardShortcuts) {
				const bindings = Object.fromEntries(Object.entries(addKeyboardShortcuts()).map(([shortcut, method]) => {
					return [shortcut, () => method({ editor })];
				}));
				defaultBindings = {
					...defaultBindings,
					...bindings
				};
			}
			const keyMapPlugin = keymap(defaultBindings);
			plugins.push(keyMapPlugin);
			const addInputRules = getExtensionField(extension, "addInputRules", context);
			if (isExtensionRulesEnabled(extension, editor.options.enableInputRules) && addInputRules) inputRules.push(...addInputRules());
			const addPasteRules = getExtensionField(extension, "addPasteRules", context);
			if (isExtensionRulesEnabled(extension, editor.options.enablePasteRules) && addPasteRules) pasteRules.push(...addPasteRules());
			const addProseMirrorPlugins = getExtensionField(extension, "addProseMirrorPlugins", context);
			if (addProseMirrorPlugins) {
				const proseMirrorPlugins = addProseMirrorPlugins();
				plugins.push(...proseMirrorPlugins);
			}
			return plugins;
		}).flat();
		return [
			inputRulesPlugin({
				editor,
				rules: inputRules
			}),
			...pasteRulesPlugin({
				editor,
				rules: pasteRules
			}),
			...allPlugins
		];
	}
	/**
	* Get all attributes from the extensions.
	* @returns An array of attributes
	*/
	get attributes() {
		return getAttributesFromExtensions(this.extensions);
	}
	/**
	* Get all node views from the extensions.
	* @returns An object with all node views where the key is the node name and the value is the node view function
	*/
	get nodeViews() {
		const { editor } = this;
		const { nodeExtensions } = splitExtensions(this.extensions);
		return Object.fromEntries(nodeExtensions.filter((extension) => !!getExtensionField(extension, "addNodeView")).map((extension) => {
			const extensionAttributes = this.attributes.filter((attribute) => attribute.type === extension.name);
			const addNodeView = getExtensionField(extension, "addNodeView", {
				name: extension.name,
				options: extension.options,
				storage: extension.storage,
				editor,
				type: getNodeType(extension.name, this.schema)
			});
			if (!addNodeView) return [];
			const nodeview = (node, view, getPos, decorations, innerDecorations) => {
				const HTMLAttributes = getRenderedAttributes(node, extensionAttributes);
				return addNodeView()({
					node,
					view,
					getPos,
					decorations,
					innerDecorations,
					editor,
					extension,
					HTMLAttributes
				});
			};
			return [extension.name, nodeview];
		}));
	}
	/**
	* Go through all extensions, create extension storages & setup marks
	* & bind editor event listener.
	*/
	setupExtensions() {
		this.extensions.forEach((extension) => {
			var _a;
			this.editor.extensionStorage[extension.name] = extension.storage;
			const context = {
				name: extension.name,
				options: extension.options,
				storage: extension.storage,
				editor: this.editor,
				type: getSchemaTypeByName(extension.name, this.schema)
			};
			if (extension.type === "mark") {
				if ((_a = callOrReturn(getExtensionField(extension, "keepOnSplit", context))) !== null && _a !== void 0 ? _a : true) this.splittableMarks.push(extension.name);
			}
			const onBeforeCreate = getExtensionField(extension, "onBeforeCreate", context);
			const onCreate = getExtensionField(extension, "onCreate", context);
			const onUpdate = getExtensionField(extension, "onUpdate", context);
			const onSelectionUpdate = getExtensionField(extension, "onSelectionUpdate", context);
			const onTransaction = getExtensionField(extension, "onTransaction", context);
			const onFocus = getExtensionField(extension, "onFocus", context);
			const onBlur = getExtensionField(extension, "onBlur", context);
			const onDestroy = getExtensionField(extension, "onDestroy", context);
			if (onBeforeCreate) this.editor.on("beforeCreate", onBeforeCreate);
			if (onCreate) this.editor.on("create", onCreate);
			if (onUpdate) this.editor.on("update", onUpdate);
			if (onSelectionUpdate) this.editor.on("selectionUpdate", onSelectionUpdate);
			if (onTransaction) this.editor.on("transaction", onTransaction);
			if (onFocus) this.editor.on("focus", onFocus);
			if (onBlur) this.editor.on("blur", onBlur);
			if (onDestroy) this.editor.on("destroy", onDestroy);
		});
	}
};
/**
* The Extension class is the base class for all extensions.
* @see https://tiptap.dev/api/extensions#create-a-new-extension
*/
var Extension = class Extension {
	constructor(config = {}) {
		this.type = "extension";
		this.name = "extension";
		this.parent = null;
		this.child = null;
		this.config = {
			name: this.name,
			defaultOptions: {}
		};
		this.config = {
			...this.config,
			...config
		};
		this.name = this.config.name;
		if (config.defaultOptions && Object.keys(config.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${this.name}".`);
		this.options = this.config.defaultOptions;
		if (this.config.addOptions) this.options = callOrReturn(getExtensionField(this, "addOptions", { name: this.name }));
		this.storage = callOrReturn(getExtensionField(this, "addStorage", {
			name: this.name,
			options: this.options
		})) || {};
	}
	static create(config = {}) {
		return new Extension(config);
	}
	configure(options = {}) {
		const extension = this.extend({
			...this.config,
			addOptions: () => {
				return mergeDeep(this.options, options);
			}
		});
		extension.name = this.name;
		extension.parent = this.parent;
		return extension;
	}
	extend(extendedConfig = {}) {
		const extension = new Extension({
			...this.config,
			...extendedConfig
		});
		extension.parent = this;
		this.child = extension;
		extension.name = extendedConfig.name ? extendedConfig.name : extension.parent.name;
		if (extendedConfig.defaultOptions && Object.keys(extendedConfig.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${extension.name}".`);
		extension.options = callOrReturn(getExtensionField(extension, "addOptions", { name: extension.name }));
		extension.storage = callOrReturn(getExtensionField(extension, "addStorage", {
			name: extension.name,
			options: extension.options
		}));
		return extension;
	}
};
/**
* Gets the text between two positions in a Prosemirror node
* and serializes it using the given text serializers and block separator (see getText)
* @param startNode The Prosemirror node to start from
* @param range The range of the text to get
* @param options Options for the text serializer & block separator
* @returns The text between the two positions
*/
function getTextBetween(startNode, range, options) {
	const { from, to } = range;
	const { blockSeparator = "\n\n", textSerializers = {} } = options || {};
	let text = "";
	startNode.nodesBetween(from, to, (node, pos, parent, index) => {
		var _a;
		if (node.isBlock && pos > from) text += blockSeparator;
		const textSerializer = textSerializers === null || textSerializers === void 0 ? void 0 : textSerializers[node.type.name];
		if (textSerializer) {
			if (parent) text += textSerializer({
				node,
				pos,
				parent,
				index,
				range
			});
			return false;
		}
		if (node.isText) text += (_a = node === null || node === void 0 ? void 0 : node.text) === null || _a === void 0 ? void 0 : _a.slice(Math.max(from, pos) - pos, to - pos);
	});
	return text;
}
/**
* Find text serializers `toText` in a Prosemirror schema
* @param schema The Prosemirror schema to search in
* @returns A record of text serializers by node name
*/
function getTextSerializersFromSchema(schema) {
	return Object.fromEntries(Object.entries(schema.nodes).filter(([, node]) => node.spec.toText).map(([name, node]) => [name, node.spec.toText]));
}
var ClipboardTextSerializer = Extension.create({
	name: "clipboardTextSerializer",
	addOptions() {
		return { blockSeparator: void 0 };
	},
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("clipboardTextSerializer"),
			props: { clipboardTextSerializer: () => {
				const { editor } = this;
				const { state, schema } = editor;
				const { doc, selection } = state;
				const { ranges } = selection;
				const from = Math.min(...ranges.map((range) => range.$from.pos));
				const to = Math.max(...ranges.map((range) => range.$to.pos));
				const textSerializers = getTextSerializersFromSchema(schema);
				return getTextBetween(doc, {
					from,
					to
				}, {
					...this.options.blockSeparator !== void 0 ? { blockSeparator: this.options.blockSeparator } : {},
					textSerializers
				});
			} }
		})];
	}
});
var blur = () => ({ editor, view }) => {
	requestAnimationFrame(() => {
		var _a;
		if (!editor.isDestroyed) {
			view.dom.blur();
			(_a = window === null || window === void 0 ? void 0 : window.getSelection()) === null || _a === void 0 || _a.removeAllRanges();
		}
	});
	return true;
};
var clearContent = (emitUpdate = false) => ({ commands }) => {
	return commands.setContent("", emitUpdate);
};
var clearNodes = () => ({ state, tr, dispatch }) => {
	const { selection } = tr;
	const { ranges } = selection;
	if (!dispatch) return true;
	ranges.forEach(({ $from, $to }) => {
		state.doc.nodesBetween($from.pos, $to.pos, (node, pos) => {
			if (node.type.isText) return;
			const { doc, mapping } = tr;
			const $mappedFrom = doc.resolve(mapping.map(pos));
			const $mappedTo = doc.resolve(mapping.map(pos + node.nodeSize));
			const nodeRange = $mappedFrom.blockRange($mappedTo);
			if (!nodeRange) return;
			const targetLiftDepth = liftTarget(nodeRange);
			if (node.type.isTextblock) {
				const { defaultType } = $mappedFrom.parent.contentMatchAt($mappedFrom.index());
				tr.setNodeMarkup(nodeRange.start, defaultType);
			}
			if (targetLiftDepth || targetLiftDepth === 0) tr.lift(nodeRange, targetLiftDepth);
		});
	});
	return true;
};
var command = (fn) => (props) => {
	return fn(props);
};
var createParagraphNear = () => ({ state, dispatch }) => {
	return createParagraphNear$1(state, dispatch);
};
var cut = (originRange, targetPos) => ({ editor, tr }) => {
	const { state } = editor;
	const contentSlice = state.doc.slice(originRange.from, originRange.to);
	tr.deleteRange(originRange.from, originRange.to);
	const newPos = tr.mapping.map(targetPos);
	tr.insert(newPos, contentSlice.content);
	tr.setSelection(new TextSelection(tr.doc.resolve(Math.max(newPos - 1, 0))));
	return true;
};
var deleteCurrentNode = () => ({ tr, dispatch }) => {
	const { selection } = tr;
	const currentNode = selection.$anchor.node();
	if (currentNode.content.size > 0) return false;
	const $pos = tr.selection.$anchor;
	for (let depth = $pos.depth; depth > 0; depth -= 1) if ($pos.node(depth).type === currentNode.type) {
		if (dispatch) {
			const from = $pos.before(depth);
			const to = $pos.after(depth);
			tr.delete(from, to).scrollIntoView();
		}
		return true;
	}
	return false;
};
var deleteNode = (typeOrName) => ({ tr, state, dispatch }) => {
	const type = getNodeType(typeOrName, state.schema);
	const $pos = tr.selection.$anchor;
	for (let depth = $pos.depth; depth > 0; depth -= 1) if ($pos.node(depth).type === type) {
		if (dispatch) {
			const from = $pos.before(depth);
			const to = $pos.after(depth);
			tr.delete(from, to).scrollIntoView();
		}
		return true;
	}
	return false;
};
var deleteRange = (range) => ({ tr, dispatch }) => {
	const { from, to } = range;
	if (dispatch) tr.delete(from, to);
	return true;
};
var deleteSelection = () => ({ state, dispatch }) => {
	return deleteSelection$1(state, dispatch);
};
var enter = () => ({ commands }) => {
	return commands.keyboardShortcut("Enter");
};
var exitCode = () => ({ state, dispatch }) => {
	return exitCode$1(state, dispatch);
};
/**
* Check if object1 includes object2
* @param object1 Object
* @param object2 Object
*/
function objectIncludes(object1, object2, options = { strict: true }) {
	const keys = Object.keys(object2);
	if (!keys.length) return true;
	return keys.every((key) => {
		if (options.strict) return object2[key] === object1[key];
		if (isRegExp(object2[key])) return object2[key].test(object1[key]);
		return object2[key] === object1[key];
	});
}
function findMarkInSet(marks, type, attributes = {}) {
	return marks.find((item) => {
		return item.type === type && objectIncludes(Object.fromEntries(Object.keys(attributes).map((k) => [k, item.attrs[k]])), attributes);
	});
}
function isMarkInSet(marks, type, attributes = {}) {
	return !!findMarkInSet(marks, type, attributes);
}
/**
* Get the range of a mark at a resolved position.
*/
function getMarkRange($pos, type, attributes) {
	var _a;
	if (!$pos || !type) return;
	let start = $pos.parent.childAfter($pos.parentOffset);
	if (!start.node || !start.node.marks.some((mark) => mark.type === type)) start = $pos.parent.childBefore($pos.parentOffset);
	if (!start.node || !start.node.marks.some((mark) => mark.type === type)) return;
	attributes = attributes || ((_a = start.node.marks[0]) === null || _a === void 0 ? void 0 : _a.attrs);
	if (!findMarkInSet([...start.node.marks], type, attributes)) return;
	let startIndex = start.index;
	let startPos = $pos.start() + start.offset;
	let endIndex = startIndex + 1;
	let endPos = startPos + start.node.nodeSize;
	while (startIndex > 0 && isMarkInSet([...$pos.parent.child(startIndex - 1).marks], type, attributes)) {
		startIndex -= 1;
		startPos -= $pos.parent.child(startIndex).nodeSize;
	}
	while (endIndex < $pos.parent.childCount && isMarkInSet([...$pos.parent.child(endIndex).marks], type, attributes)) {
		endPos += $pos.parent.child(endIndex).nodeSize;
		endIndex += 1;
	}
	return {
		from: startPos,
		to: endPos
	};
}
function getMarkType(nameOrType, schema) {
	if (typeof nameOrType === "string") {
		if (!schema.marks[nameOrType]) throw Error(`There is no mark type named '${nameOrType}'. Maybe you forgot to add the extension?`);
		return schema.marks[nameOrType];
	}
	return nameOrType;
}
var extendMarkRange = (typeOrName, attributes = {}) => ({ tr, state, dispatch }) => {
	const type = getMarkType(typeOrName, state.schema);
	const { doc, selection } = tr;
	const { $from, from, to } = selection;
	if (dispatch) {
		const range = getMarkRange($from, type, attributes);
		if (range && range.from <= from && range.to >= to) {
			const newSelection = TextSelection.create(doc, range.from, range.to);
			tr.setSelection(newSelection);
		}
	}
	return true;
};
var first = (commands) => (props) => {
	const items = typeof commands === "function" ? commands(props) : commands;
	for (let i = 0; i < items.length; i += 1) if (items[i](props)) return true;
	return false;
};
function isTextSelection(value) {
	return value instanceof TextSelection;
}
function minMax(value = 0, min = 0, max = 0) {
	return Math.min(Math.max(value, min), max);
}
function resolveFocusPosition(doc, position = null) {
	if (!position) return null;
	const selectionAtStart = Selection.atStart(doc);
	const selectionAtEnd = Selection.atEnd(doc);
	if (position === "start" || position === true) return selectionAtStart;
	if (position === "end") return selectionAtEnd;
	const minPos = selectionAtStart.from;
	const maxPos = selectionAtEnd.to;
	if (position === "all") return TextSelection.create(doc, minMax(0, minPos, maxPos), minMax(doc.content.size, minPos, maxPos));
	return TextSelection.create(doc, minMax(position, minPos, maxPos), minMax(position, minPos, maxPos));
}
function isAndroid() {
	return navigator.platform === "Android" || /android/i.test(navigator.userAgent);
}
function isiOS() {
	return [
		"iPad Simulator",
		"iPhone Simulator",
		"iPod Simulator",
		"iPad",
		"iPhone",
		"iPod"
	].includes(navigator.platform) || navigator.userAgent.includes("Mac") && "ontouchend" in document;
}
/**
* Detects if the current browser is Safari (but not iOS Safari or Chrome).
* @returns `true` if the browser is Safari, `false` otherwise.
* @example
* if (isSafari()) {
*   // Safari-specific handling
* }
*/
function isSafari() {
	return typeof navigator !== "undefined" ? /^((?!chrome|android).)*safari/i.test(navigator.userAgent) : false;
}
var focus = (position = null, options = {}) => ({ editor, view, tr, dispatch }) => {
	options = {
		scrollIntoView: true,
		...options
	};
	const delayedFocus = () => {
		if (isiOS() || isAndroid()) view.dom.focus();
		requestAnimationFrame(() => {
			if (!editor.isDestroyed) {
				view.focus();
				if (isSafari() && !isiOS() && !isAndroid()) view.dom.focus({ preventScroll: true });
			}
		});
	};
	if (view.hasFocus() && position === null || position === false) return true;
	if (dispatch && position === null && !isTextSelection(editor.state.selection)) {
		delayedFocus();
		return true;
	}
	const selection = resolveFocusPosition(tr.doc, position) || editor.state.selection;
	const isSameSelection = editor.state.selection.eq(selection);
	if (dispatch) {
		if (!isSameSelection) tr.setSelection(selection);
		if (isSameSelection && tr.storedMarks) tr.setStoredMarks(tr.storedMarks);
		delayedFocus();
	}
	return true;
};
var forEach = (items, fn) => (props) => {
	return items.every((item, index) => fn(item, {
		...props,
		index
	}));
};
var insertContent = (value, options) => ({ tr, commands }) => {
	return commands.insertContentAt({
		from: tr.selection.from,
		to: tr.selection.to
	}, value, options);
};
var removeWhitespaces = (node) => {
	const children = node.childNodes;
	for (let i = children.length - 1; i >= 0; i -= 1) {
		const child = children[i];
		if (child.nodeType === 3 && child.nodeValue && /^(\n\s\s|\n)$/.test(child.nodeValue)) node.removeChild(child);
		else if (child.nodeType === 1) removeWhitespaces(child);
	}
	return node;
};
function elementFromString(value) {
	const wrappedValue = `<body>${value}</body>`;
	const html = new window.DOMParser().parseFromString(wrappedValue, "text/html").body;
	return removeWhitespaces(html);
}
/**
* Takes a JSON or HTML content and creates a Prosemirror node or fragment from it.
* @param content The JSON or HTML content to create the node from
* @param schema The Prosemirror schema to use for the node
* @param options Options for the parser
* @returns The created Prosemirror node or fragment
*/
function createNodeFromContent(content, schema, options) {
	if (content instanceof Node$1 || content instanceof Fragment) return content;
	options = {
		slice: true,
		parseOptions: {},
		...options
	};
	const isJSONContent = typeof content === "object" && content !== null;
	const isTextContent = typeof content === "string";
	if (isJSONContent) try {
		if (Array.isArray(content) && content.length > 0) return Fragment.fromArray(content.map((item) => schema.nodeFromJSON(item)));
		const node = schema.nodeFromJSON(content);
		if (options.errorOnInvalidContent) node.check();
		return node;
	} catch (error) {
		if (options.errorOnInvalidContent) throw new Error("[tiptap error]: Invalid JSON content", { cause: error });
		console.warn("[tiptap warn]: Invalid content.", "Passed value:", content, "Error:", error);
		return createNodeFromContent("", schema, options);
	}
	if (isTextContent) {
		if (options.errorOnInvalidContent) {
			let hasInvalidContent = false;
			let invalidContent = "";
			const contentCheckSchema = new Schema({
				topNode: schema.spec.topNode,
				marks: schema.spec.marks,
				nodes: schema.spec.nodes.append({ __tiptap__private__unknown__catch__all__node: {
					content: "inline*",
					group: "block",
					parseDOM: [{
						tag: "*",
						getAttrs: (e) => {
							hasInvalidContent = true;
							invalidContent = typeof e === "string" ? e : e.outerHTML;
							return null;
						}
					}]
				} })
			});
			if (options.slice) DOMParser.fromSchema(contentCheckSchema).parseSlice(elementFromString(content), options.parseOptions);
			else DOMParser.fromSchema(contentCheckSchema).parse(elementFromString(content), options.parseOptions);
			if (options.errorOnInvalidContent && hasInvalidContent) throw new Error("[tiptap error]: Invalid HTML content", { cause: /* @__PURE__ */ new Error(`Invalid element found: ${invalidContent}`) });
		}
		const parser = DOMParser.fromSchema(schema);
		if (options.slice) return parser.parseSlice(elementFromString(content), options.parseOptions).content;
		return parser.parse(elementFromString(content), options.parseOptions);
	}
	return createNodeFromContent("", schema, options);
}
function selectionToInsertionEnd(tr, startLen, bias) {
	const last = tr.steps.length - 1;
	if (last < startLen) return;
	const step = tr.steps[last];
	if (!(step instanceof ReplaceStep || step instanceof ReplaceAroundStep)) return;
	const map = tr.mapping.maps[last];
	let end = 0;
	map.forEach((_from, _to, _newFrom, newTo) => {
		if (end === 0) end = newTo;
	});
	tr.setSelection(Selection.near(tr.doc.resolve(end), bias));
}
var isFragment = (nodeOrFragment) => {
	return !("type" in nodeOrFragment);
};
var insertContentAt = (position, value, options) => ({ tr, dispatch, editor }) => {
	var _a;
	if (dispatch) {
		options = {
			parseOptions: editor.options.parseOptions,
			updateSelection: true,
			applyInputRules: false,
			applyPasteRules: false,
			...options
		};
		let content;
		const emitContentError = (error) => {
			editor.emit("contentError", {
				editor,
				error,
				disableCollaboration: () => {
					if (editor.storage.collaboration) editor.storage.collaboration.isDisabled = true;
				}
			});
		};
		const parseOptions = {
			preserveWhitespace: "full",
			...options.parseOptions
		};
		if (!options.errorOnInvalidContent && !editor.options.enableContentCheck && editor.options.emitContentError) try {
			createNodeFromContent(value, editor.schema, {
				parseOptions,
				errorOnInvalidContent: true
			});
		} catch (e) {
			emitContentError(e);
		}
		try {
			content = createNodeFromContent(value, editor.schema, {
				parseOptions,
				errorOnInvalidContent: (_a = options.errorOnInvalidContent) !== null && _a !== void 0 ? _a : editor.options.enableContentCheck
			});
		} catch (e) {
			emitContentError(e);
			return false;
		}
		let { from, to } = typeof position === "number" ? {
			from: position,
			to: position
		} : {
			from: position.from,
			to: position.to
		};
		let isOnlyTextContent = true;
		let isOnlyBlockContent = true;
		(isFragment(content) ? content : [content]).forEach((node) => {
			node.check();
			isOnlyTextContent = isOnlyTextContent ? node.isText && node.marks.length === 0 : false;
			isOnlyBlockContent = isOnlyBlockContent ? node.isBlock : false;
		});
		if (from === to && isOnlyBlockContent) {
			const { parent } = tr.doc.resolve(from);
			if (parent.isTextblock && !parent.type.spec.code && !parent.childCount) {
				from -= 1;
				to += 1;
			}
		}
		let newContent;
		if (isOnlyTextContent) {
			if (Array.isArray(value)) newContent = value.map((v) => v.text || "").join("");
			else if (value instanceof Fragment) {
				let text = "";
				value.forEach((node) => {
					if (node.text) text += node.text;
				});
				newContent = text;
			} else if (typeof value === "object" && !!value && !!value.text) newContent = value.text;
			else newContent = value;
			tr.insertText(newContent, from, to);
		} else {
			newContent = content;
			tr.replaceWith(from, to, newContent);
		}
		if (options.updateSelection) selectionToInsertionEnd(tr, tr.steps.length - 1, -1);
		if (options.applyInputRules) tr.setMeta("applyInputRules", {
			from,
			text: newContent
		});
		if (options.applyPasteRules) tr.setMeta("applyPasteRules", {
			from,
			text: newContent
		});
	}
	return true;
};
var joinUp = () => ({ state, dispatch }) => {
	return joinUp$1(state, dispatch);
};
var joinDown = () => ({ state, dispatch }) => {
	return joinDown$1(state, dispatch);
};
var joinBackward = () => ({ state, dispatch }) => {
	return joinBackward$1(state, dispatch);
};
var joinForward = () => ({ state, dispatch }) => {
	return joinForward$1(state, dispatch);
};
var joinItemBackward = () => ({ state, dispatch, tr }) => {
	try {
		const point = joinPoint(state.doc, state.selection.$from.pos, -1);
		if (point === null || point === void 0) return false;
		tr.join(point, 2);
		if (dispatch) dispatch(tr);
		return true;
	} catch {
		return false;
	}
};
var joinItemForward = () => ({ state, dispatch, tr }) => {
	try {
		const point = joinPoint(state.doc, state.selection.$from.pos, 1);
		if (point === null || point === void 0) return false;
		tr.join(point, 2);
		if (dispatch) dispatch(tr);
		return true;
	} catch {
		return false;
	}
};
var joinTextblockBackward = () => ({ state, dispatch }) => {
	return joinTextblockBackward$1(state, dispatch);
};
var joinTextblockForward = () => ({ state, dispatch }) => {
	return joinTextblockForward$1(state, dispatch);
};
function isMacOS() {
	return typeof navigator !== "undefined" ? /Mac/.test(navigator.platform) : false;
}
function normalizeKeyName(name) {
	const parts = name.split(/-(?!$)/);
	let result = parts[parts.length - 1];
	if (result === "Space") result = " ";
	let alt;
	let ctrl;
	let shift;
	let meta;
	for (let i = 0; i < parts.length - 1; i += 1) {
		const mod = parts[i];
		if (/^(cmd|meta|m)$/i.test(mod)) meta = true;
		else if (/^a(lt)?$/i.test(mod)) alt = true;
		else if (/^(c|ctrl|control)$/i.test(mod)) ctrl = true;
		else if (/^s(hift)?$/i.test(mod)) shift = true;
		else if (/^mod$/i.test(mod)) {
			if (isiOS() || isMacOS()) meta = true;
			else ctrl = true;
		} else throw new Error(`Unrecognized modifier name: ${mod}`);
	}
	if (alt) result = `Alt-${result}`;
	if (ctrl) result = `Ctrl-${result}`;
	if (meta) result = `Meta-${result}`;
	if (shift) result = `Shift-${result}`;
	return result;
}
var keyboardShortcut = (name) => ({ editor, view, tr, dispatch }) => {
	const keys = normalizeKeyName(name).split(/-(?!$)/);
	const key = keys.find((item) => ![
		"Alt",
		"Ctrl",
		"Meta",
		"Shift"
	].includes(item));
	const event = new KeyboardEvent("keydown", {
		key: key === "Space" ? " " : key,
		altKey: keys.includes("Alt"),
		ctrlKey: keys.includes("Ctrl"),
		metaKey: keys.includes("Meta"),
		shiftKey: keys.includes("Shift"),
		bubbles: true,
		cancelable: true
	});
	const capturedTransaction = editor.captureTransaction(() => {
		view.someProp("handleKeyDown", (f) => f(view, event));
	});
	capturedTransaction === null || capturedTransaction === void 0 || capturedTransaction.steps.forEach((step) => {
		const newStep = step.map(tr.mapping);
		if (newStep && dispatch) tr.maybeStep(newStep);
	});
	return true;
};
function isNodeActive(state, typeOrName, attributes = {}) {
	const { from, to, empty } = state.selection;
	const type = typeOrName ? getNodeType(typeOrName, state.schema) : null;
	const nodeRanges = [];
	state.doc.nodesBetween(from, to, (node, pos) => {
		if (node.isText) return;
		const relativeFrom = Math.max(from, pos);
		const relativeTo = Math.min(to, pos + node.nodeSize);
		nodeRanges.push({
			node,
			from: relativeFrom,
			to: relativeTo
		});
	});
	const selectionRange = to - from;
	const matchedNodeRanges = nodeRanges.filter((nodeRange) => {
		if (!type) return true;
		return type.name === nodeRange.node.type.name;
	}).filter((nodeRange) => objectIncludes(nodeRange.node.attrs, attributes, { strict: false }));
	if (empty) return !!matchedNodeRanges.length;
	return matchedNodeRanges.reduce((sum, nodeRange) => sum + nodeRange.to - nodeRange.from, 0) >= selectionRange;
}
var lift = (typeOrName, attributes = {}) => ({ state, dispatch }) => {
	if (!isNodeActive(state, getNodeType(typeOrName, state.schema), attributes)) return false;
	return lift$1(state, dispatch);
};
var liftEmptyBlock = () => ({ state, dispatch }) => {
	return liftEmptyBlock$1(state, dispatch);
};
var liftListItem = (typeOrName) => ({ state, dispatch }) => {
	return liftListItem$1(getNodeType(typeOrName, state.schema))(state, dispatch);
};
var newlineInCode = () => ({ state, dispatch }) => {
	return newlineInCode$1(state, dispatch);
};
/**
* Get the type of a schema item by its name.
* @param name The name of the schema item
* @param schema The Prosemiror schema to search in
* @returns The type of the schema item (`node` or `mark`), or null if it doesn't exist
*/
function getSchemaTypeNameByName(name, schema) {
	if (schema.nodes[name]) return "node";
	if (schema.marks[name]) return "mark";
	return null;
}
/**
* Remove a property or an array of properties from an object
* @param obj Object
* @param key Key to remove
*/
function deleteProps(obj, propOrProps) {
	const props = typeof propOrProps === "string" ? [propOrProps] : propOrProps;
	return Object.keys(obj).reduce((newObj, prop) => {
		if (!props.includes(prop)) newObj[prop] = obj[prop];
		return newObj;
	}, {});
}
var resetAttributes = (typeOrName, attributes) => ({ tr, state, dispatch }) => {
	let nodeType = null;
	let markType = null;
	const schemaType = getSchemaTypeNameByName(typeof typeOrName === "string" ? typeOrName : typeOrName.name, state.schema);
	if (!schemaType) return false;
	if (schemaType === "node") nodeType = getNodeType(typeOrName, state.schema);
	if (schemaType === "mark") markType = getMarkType(typeOrName, state.schema);
	if (dispatch) tr.selection.ranges.forEach((range) => {
		state.doc.nodesBetween(range.$from.pos, range.$to.pos, (node, pos) => {
			if (nodeType && nodeType === node.type) tr.setNodeMarkup(pos, void 0, deleteProps(node.attrs, attributes));
			if (markType && node.marks.length) node.marks.forEach((mark) => {
				if (markType === mark.type) tr.addMark(pos, pos + node.nodeSize, markType.create(deleteProps(mark.attrs, attributes)));
			});
		});
	});
	return true;
};
var scrollIntoView = () => ({ tr, dispatch }) => {
	if (dispatch) tr.scrollIntoView();
	return true;
};
var selectAll = () => ({ tr, dispatch }) => {
	if (dispatch) {
		const selection = new AllSelection(tr.doc);
		tr.setSelection(selection);
	}
	return true;
};
var selectNodeBackward = () => ({ state, dispatch }) => {
	return selectNodeBackward$1(state, dispatch);
};
var selectNodeForward = () => ({ state, dispatch }) => {
	return selectNodeForward$1(state, dispatch);
};
var selectParentNode = () => ({ state, dispatch }) => {
	return selectParentNode$1(state, dispatch);
};
var selectTextblockEnd = () => ({ state, dispatch }) => {
	return selectTextblockEnd$1(state, dispatch);
};
var selectTextblockStart = () => ({ state, dispatch }) => {
	return selectTextblockStart$1(state, dispatch);
};
/**
* Create a new Prosemirror document node from content.
* @param content The JSON or HTML content to create the document from
* @param schema The Prosemirror schema to use for the document
* @param parseOptions Options for the parser
* @returns The created Prosemirror document node
*/
function createDocument(content, schema, parseOptions = {}, options = {}) {
	return createNodeFromContent(content, schema, {
		slice: false,
		parseOptions,
		errorOnInvalidContent: options.errorOnInvalidContent
	});
}
var setContent = (content, emitUpdate = false, parseOptions = {}, options = {}) => ({ editor, tr, dispatch, commands }) => {
	var _a, _b;
	const { doc } = tr;
	if (parseOptions.preserveWhitespace !== "full") {
		const document = createDocument(content, editor.schema, parseOptions, { errorOnInvalidContent: (_a = options.errorOnInvalidContent) !== null && _a !== void 0 ? _a : editor.options.enableContentCheck });
		if (dispatch) tr.replaceWith(0, doc.content.size, document).setMeta("preventUpdate", !emitUpdate);
		return true;
	}
	if (dispatch) tr.setMeta("preventUpdate", !emitUpdate);
	return commands.insertContentAt({
		from: 0,
		to: doc.content.size
	}, content, {
		parseOptions,
		errorOnInvalidContent: (_b = options.errorOnInvalidContent) !== null && _b !== void 0 ? _b : editor.options.enableContentCheck
	});
};
function getMarkAttributes(state, typeOrName) {
	const type = getMarkType(typeOrName, state.schema);
	const { from, to, empty } = state.selection;
	const marks = [];
	if (empty) {
		if (state.storedMarks) marks.push(...state.storedMarks);
		marks.push(...state.selection.$head.marks());
	} else state.doc.nodesBetween(from, to, (node) => {
		marks.push(...node.marks);
	});
	const mark = marks.find((markItem) => markItem.type.name === type.name);
	if (!mark) return {};
	return { ...mark.attrs };
}
/**
* Returns a new `Transform` based on all steps of the passed transactions.
* @param oldDoc The Prosemirror node to start from
* @param transactions The transactions to combine
* @returns A new `Transform` with all steps of the passed transactions
*/
function combineTransactionSteps(oldDoc, transactions) {
	const transform = new Transform(oldDoc);
	transactions.forEach((transaction) => {
		transaction.steps.forEach((step) => {
			transform.step(step);
		});
	});
	return transform;
}
/**
* Gets the default block type at a given match
* @param match The content match to get the default block type from
* @returns The default block type or null
*/
function defaultBlockAt(match) {
	for (let i = 0; i < match.edgeCount; i += 1) {
		const { type } = match.edge(i);
		if (type.isTextblock && !type.hasRequiredAttrs()) return type;
	}
	return null;
}
/**
* Find children inside a Prosemirror node that match a predicate.
* @param node The Prosemirror node to search in
* @param predicate The predicate to match
* @returns An array of nodes with their positions
*/
function findChildren(node, predicate) {
	const nodesWithPos = [];
	node.descendants((child, pos) => {
		if (predicate(child)) nodesWithPos.push({
			node: child,
			pos
		});
	});
	return nodesWithPos;
}
/**
* Same as `findChildren` but searches only within a `range`.
* @param node The Prosemirror node to search in
* @param range The range to search in
* @param predicate The predicate to match
* @returns An array of nodes with their positions
*/
function findChildrenInRange(node, range, predicate) {
	const nodesWithPos = [];
	node.nodesBetween(range.from, range.to, (child, pos) => {
		if (predicate(child)) nodesWithPos.push({
			node: child,
			pos
		});
	});
	return nodesWithPos;
}
/**
* Finds the closest parent node to a resolved position that matches a predicate.
* @param $pos The resolved position to search from
* @param predicate The predicate to match
* @returns The closest parent node to the resolved position that matches the predicate
* @example ```js
* findParentNodeClosestToPos($from, node => node.type.name === 'paragraph')
* ```
*/
function findParentNodeClosestToPos($pos, predicate) {
	for (let i = $pos.depth; i > 0; i -= 1) {
		const node = $pos.node(i);
		if (predicate(node)) return {
			pos: i > 0 ? $pos.before(i) : 0,
			start: $pos.start(i),
			depth: i,
			node
		};
	}
}
/**
* Finds the closest parent node to the current selection that matches a predicate.
* @param predicate The predicate to match
* @returns A command that finds the closest parent node to the current selection that matches the predicate
* @example ```js
* findParentNode(node => node.type.name === 'paragraph')
* ```
*/
function findParentNode(predicate) {
	return (selection) => findParentNodeClosestToPos(selection.$from, predicate);
}
function getSchema(extensions, editor) {
	return getSchemaByResolvedExtensions(ExtensionManager.resolve(extensions), editor);
}
/**
* Generate HTML from a JSONContent
* @param doc The JSONContent to generate HTML from
* @param extensions The extensions to use for the schema
* @returns The generated HTML
*/
function generateHTML(doc, extensions) {
	const schema = getSchema(extensions);
	return getHTMLFromFragment(Node$1.fromJSON(schema, doc).content, schema);
}
/**
* Generate JSONContent from HTML
* @param html The HTML to generate JSONContent from
* @param extensions The extensions to use for the schema
* @returns The generated JSONContent
*/
function generateJSON(html, extensions) {
	const schema = getSchema(extensions);
	const dom = elementFromString(html);
	return DOMParser.fromSchema(schema).parse(dom).toJSON();
}
/**
* Gets the text of a Prosemirror node
* @param node The Prosemirror node
* @param options Options for the text serializer & block separator
* @returns The text of the node
* @example ```js
* const text = getText(node, { blockSeparator: '\n' })
* ```
*/
function getText(node, options) {
	return getTextBetween(node, {
		from: 0,
		to: node.content.size
	}, options);
}
/**
* Generate raw text from a JSONContent
* @param doc The JSONContent to generate text from
* @param extensions The extensions to use for the schema
* @param options Options for the text generation f.e. blockSeparator or textSerializers
* @returns The generated text
*/
function generateText(doc, extensions, options) {
	const { blockSeparator = "\n\n", textSerializers = {} } = options || {};
	const schema = getSchema(extensions);
	return getText(Node$1.fromJSON(schema, doc), {
		blockSeparator,
		textSerializers: {
			...getTextSerializersFromSchema(schema),
			...textSerializers
		}
	});
}
function getNodeAttributes(state, typeOrName) {
	const type = getNodeType(typeOrName, state.schema);
	const { from, to } = state.selection;
	const nodes = [];
	state.doc.nodesBetween(from, to, (node) => {
		nodes.push(node);
	});
	const node = nodes.reverse().find((nodeItem) => nodeItem.type.name === type.name);
	if (!node) return {};
	return { ...node.attrs };
}
/**
* Get node or mark attributes by type or name on the current editor state
* @param state The current editor state
* @param typeOrName The node or mark type or name
* @returns The attributes of the node or mark or an empty object
*/
function getAttributes(state, typeOrName) {
	const schemaType = getSchemaTypeNameByName(typeof typeOrName === "string" ? typeOrName : typeOrName.name, state.schema);
	if (schemaType === "node") return getNodeAttributes(state, typeOrName);
	if (schemaType === "mark") return getMarkAttributes(state, typeOrName);
	return {};
}
/**
* Removes duplicated values within an array.
* Supports numbers, strings and objects.
*/
function removeDuplicates(array, by = JSON.stringify) {
	const seen = {};
	return array.filter((item) => {
		const key = by(item);
		return Object.prototype.hasOwnProperty.call(seen, key) ? false : seen[key] = true;
	});
}
/**
* Removes duplicated ranges and ranges that are
* fully captured by other ranges.
*/
function simplifyChangedRanges(changes) {
	const uniqueChanges = removeDuplicates(changes);
	return uniqueChanges.length === 1 ? uniqueChanges : uniqueChanges.filter((change, index) => {
		return !uniqueChanges.filter((_, i) => i !== index).some((otherChange) => {
			return change.oldRange.from >= otherChange.oldRange.from && change.oldRange.to <= otherChange.oldRange.to && change.newRange.from >= otherChange.newRange.from && change.newRange.to <= otherChange.newRange.to;
		});
	});
}
/**
* Returns a list of changed ranges
* based on the first and last state of all steps.
*/
function getChangedRanges(transform) {
	const { mapping, steps } = transform;
	const changes = [];
	mapping.maps.forEach((stepMap, index) => {
		const ranges = [];
		if (!stepMap.ranges.length) {
			const { from, to } = steps[index];
			if (from === void 0 || to === void 0) return;
			ranges.push({
				from,
				to
			});
		} else stepMap.forEach((from, to) => {
			ranges.push({
				from,
				to
			});
		});
		ranges.forEach(({ from, to }) => {
			const newStart = mapping.slice(index).map(from, -1);
			const newEnd = mapping.slice(index).map(to);
			const oldStart = mapping.invert().map(newStart, -1);
			const oldEnd = mapping.invert().map(newEnd);
			changes.push({
				oldRange: {
					from: oldStart,
					to: oldEnd
				},
				newRange: {
					from: newStart,
					to: newEnd
				}
			});
		});
	});
	return simplifyChangedRanges(changes);
}
function getDebugJSON(node, startOffset = 0) {
	const increment = node.type === node.type.schema.topNodeType ? 0 : 1;
	const from = startOffset;
	const to = from + node.nodeSize;
	const marks = node.marks.map((mark) => {
		const output = { type: mark.type.name };
		if (Object.keys(mark.attrs).length) output.attrs = { ...mark.attrs };
		return output;
	});
	const attrs = { ...node.attrs };
	const output = {
		type: node.type.name,
		from,
		to
	};
	if (Object.keys(attrs).length) output.attrs = attrs;
	if (marks.length) output.marks = marks;
	if (node.content.childCount) {
		output.content = [];
		node.forEach((child, offset) => {
			var _a;
			(_a = output.content) === null || _a === void 0 || _a.push(getDebugJSON(child, startOffset + offset + increment));
		});
	}
	if (node.text) output.text = node.text;
	return output;
}
function getMarksBetween(from, to, doc) {
	const marks = [];
	if (from === to) doc.resolve(from).marks().forEach((mark) => {
		const range = getMarkRange(doc.resolve(from), mark.type);
		if (!range) return;
		marks.push({
			mark,
			...range
		});
	});
	else doc.nodesBetween(from, to, (node, pos) => {
		if (!node || (node === null || node === void 0 ? void 0 : node.nodeSize) === void 0) return;
		marks.push(...node.marks.map((mark) => ({
			from: pos,
			to: pos + node.nodeSize,
			mark
		})));
	});
	return marks;
}
/**
* Finds the first node of a given type or name in the current selection.
* @param state The editor state.
* @param typeOrName The node type or name.
* @param pos The position to start searching from.
* @param maxDepth The maximum depth to search.
* @returns The node and the depth as an array.
*/
var getNodeAtPosition = (state, typeOrName, pos, maxDepth = 20) => {
	const $pos = state.doc.resolve(pos);
	let currentDepth = maxDepth;
	let node = null;
	while (currentDepth > 0 && node === null) {
		const currentNode = $pos.node(currentDepth);
		if ((currentNode === null || currentNode === void 0 ? void 0 : currentNode.type.name) === typeOrName) node = currentNode;
		else currentDepth -= 1;
	}
	return [node, currentDepth];
};
/**
* Return attributes of an extension that should be splitted by keepOnSplit flag
* @param extensionAttributes Array of extension attributes
* @param typeName The type of the extension
* @param attributes The attributes of the extension
* @returns The splitted attributes
*/
function getSplittedAttributes(extensionAttributes, typeName, attributes) {
	return Object.fromEntries(Object.entries(attributes).filter(([name]) => {
		const extensionAttribute = extensionAttributes.find((item) => {
			return item.type === typeName && item.name === name;
		});
		if (!extensionAttribute) return false;
		return extensionAttribute.attribute.keepOnSplit;
	}));
}
function isMarkActive(state, typeOrName, attributes = {}) {
	const { empty, ranges } = state.selection;
	const type = typeOrName ? getMarkType(typeOrName, state.schema) : null;
	if (empty) return !!(state.storedMarks || state.selection.$from.marks()).filter((mark) => {
		if (!type) return true;
		return type.name === mark.type.name;
	}).find((mark) => objectIncludes(mark.attrs, attributes, { strict: false }));
	let selectionRange = 0;
	const markRanges = [];
	ranges.forEach(({ $from, $to }) => {
		const from = $from.pos;
		const to = $to.pos;
		state.doc.nodesBetween(from, to, (node, pos) => {
			if (!node.isText && !node.marks.length) return;
			const relativeFrom = Math.max(from, pos);
			const relativeTo = Math.min(to, pos + node.nodeSize);
			const range = relativeTo - relativeFrom;
			selectionRange += range;
			markRanges.push(...node.marks.map((mark) => ({
				mark,
				from: relativeFrom,
				to: relativeTo
			})));
		});
	});
	if (selectionRange === 0) return false;
	const matchedRange = markRanges.filter((markRange) => {
		if (!type) return true;
		return type.name === markRange.mark.type.name;
	}).filter((markRange) => objectIncludes(markRange.mark.attrs, attributes, { strict: false })).reduce((sum, markRange) => sum + markRange.to - markRange.from, 0);
	const excludedRange = markRanges.filter((markRange) => {
		if (!type) return true;
		return markRange.mark.type !== type && markRange.mark.type.excludes(type);
	}).reduce((sum, markRange) => sum + markRange.to - markRange.from, 0);
	return (matchedRange > 0 ? matchedRange + excludedRange : matchedRange) >= selectionRange;
}
function isActive(state, name, attributes = {}) {
	if (!name) return isNodeActive(state, null, attributes) || isMarkActive(state, null, attributes);
	const schemaType = getSchemaTypeNameByName(name, state.schema);
	if (schemaType === "node") return isNodeActive(state, name, attributes);
	if (schemaType === "mark") return isMarkActive(state, name, attributes);
	return false;
}
var isAtEndOfNode = (state, nodeType) => {
	const { $from, $to, $anchor } = state.selection;
	if (nodeType) {
		const parentNode = findParentNode((node) => node.type.name === nodeType)(state.selection);
		if (!parentNode) return false;
		const $parentPos = state.doc.resolve(parentNode.pos + 1);
		if ($anchor.pos + 1 === $parentPos.end()) return true;
		return false;
	}
	if ($to.parentOffset < $to.parent.nodeSize - 2 || $from.pos !== $to.pos) return false;
	return true;
};
var isAtStartOfNode = (state) => {
	const { $from, $to } = state.selection;
	if ($from.parentOffset > 0 || $from.pos !== $to.pos) return false;
	return true;
};
function isList(name, extensions) {
	const { nodeExtensions } = splitExtensions(extensions);
	const extension = nodeExtensions.find((item) => item.name === name);
	if (!extension) return false;
	const group = callOrReturn(getExtensionField(extension, "group", {
		name: extension.name,
		options: extension.options,
		storage: extension.storage
	}));
	if (typeof group !== "string") return false;
	return group.split(" ").includes("list");
}
/**
* Returns true if the given prosemirror node is empty.
*/
function isNodeEmpty(node, { checkChildren = true, ignoreWhitespace = false } = {}) {
	var _a;
	if (ignoreWhitespace) {
		if (node.type.name === "hardBreak") return true;
		if (node.isText) return /^\s*$/m.test((_a = node.text) !== null && _a !== void 0 ? _a : "");
	}
	if (node.isText) return !node.text;
	if (node.isAtom || node.isLeaf) return false;
	if (node.content.childCount === 0) return true;
	if (checkChildren) {
		let isContentEmpty = true;
		node.content.forEach((childNode) => {
			if (isContentEmpty === false) return;
			if (!isNodeEmpty(childNode, {
				ignoreWhitespace,
				checkChildren
			})) isContentEmpty = false;
		});
		return isContentEmpty;
	}
	return false;
}
function isNodeSelection(value) {
	return value instanceof NodeSelection;
}
function posToDOMRect(view, from, to) {
	const minPos = 0;
	const maxPos = view.state.doc.content.size;
	const resolvedFrom = minMax(from, minPos, maxPos);
	const resolvedEnd = minMax(to, minPos, maxPos);
	const start = view.coordsAtPos(resolvedFrom);
	const end = view.coordsAtPos(resolvedEnd, -1);
	const top = Math.min(start.top, end.top);
	const bottom = Math.max(start.bottom, end.bottom);
	const left = Math.min(start.left, end.left);
	const right = Math.max(start.right, end.right);
	const data = {
		top,
		bottom,
		left,
		right,
		width: right - left,
		height: bottom - top,
		x: left,
		y: top
	};
	return {
		...data,
		toJSON: () => data
	};
}
/**
* The actual implementation of the rewriteUnknownContent function
*/
function rewriteUnknownContentInner({ json, validMarks, validNodes, options, rewrittenContent = [] }) {
	if (json.marks && Array.isArray(json.marks)) json.marks = json.marks.filter((mark) => {
		const name = typeof mark === "string" ? mark : mark.type;
		if (validMarks.has(name)) return true;
		rewrittenContent.push({
			original: JSON.parse(JSON.stringify(mark)),
			unsupported: name
		});
		return false;
	});
	if (json.content && Array.isArray(json.content)) json.content = json.content.map((value) => rewriteUnknownContentInner({
		json: value,
		validMarks,
		validNodes,
		options,
		rewrittenContent
	}).json).filter((a) => a !== null && a !== void 0);
	if (json.type && !validNodes.has(json.type)) {
		rewrittenContent.push({
			original: JSON.parse(JSON.stringify(json)),
			unsupported: json.type
		});
		if (json.content && Array.isArray(json.content) && (options === null || options === void 0 ? void 0 : options.fallbackToParagraph) !== false) {
			json.type = "paragraph";
			return {
				json,
				rewrittenContent
			};
		}
		return {
			json: null,
			rewrittenContent
		};
	}
	return {
		json,
		rewrittenContent
	};
}
/**
* Rewrite unknown nodes and marks within JSON content
* Allowing for user within the editor
*/
function rewriteUnknownContent(json, schema, options) {
	return rewriteUnknownContentInner({
		json,
		validNodes: new Set(Object.keys(schema.nodes)),
		validMarks: new Set(Object.keys(schema.marks)),
		options
	});
}
function canSetMark(state, tr, newMarkType) {
	var _a;
	const { selection } = tr;
	let cursor = null;
	if (isTextSelection(selection)) cursor = selection.$cursor;
	if (cursor) {
		const currentMarks = (_a = state.storedMarks) !== null && _a !== void 0 ? _a : cursor.marks();
		return !!newMarkType.isInSet(currentMarks) || !currentMarks.some((mark) => mark.type.excludes(newMarkType));
	}
	const { ranges } = selection;
	return ranges.some(({ $from, $to }) => {
		let someNodeSupportsMark = $from.depth === 0 ? state.doc.inlineContent && state.doc.type.allowsMarkType(newMarkType) : false;
		state.doc.nodesBetween($from.pos, $to.pos, (node, _pos, parent) => {
			if (someNodeSupportsMark) return false;
			if (node.isInline) {
				const parentAllowsMarkType = !parent || parent.type.allowsMarkType(newMarkType);
				const currentMarksAllowMarkType = !!newMarkType.isInSet(node.marks) || !node.marks.some((otherMark) => otherMark.type.excludes(newMarkType));
				someNodeSupportsMark = parentAllowsMarkType && currentMarksAllowMarkType;
			}
			return !someNodeSupportsMark;
		});
		return someNodeSupportsMark;
	});
}
var setMark = (typeOrName, attributes = {}) => ({ tr, state, dispatch }) => {
	const { selection } = tr;
	const { empty, ranges } = selection;
	const type = getMarkType(typeOrName, state.schema);
	if (dispatch) {
		if (empty) {
			const oldAttributes = getMarkAttributes(state, type);
			tr.addStoredMark(type.create({
				...oldAttributes,
				...attributes
			}));
		} else ranges.forEach((range) => {
			const from = range.$from.pos;
			const to = range.$to.pos;
			state.doc.nodesBetween(from, to, (node, pos) => {
				const trimmedFrom = Math.max(pos, from);
				const trimmedTo = Math.min(pos + node.nodeSize, to);
				if (node.marks.find((mark) => mark.type === type)) node.marks.forEach((mark) => {
					if (type === mark.type) tr.addMark(trimmedFrom, trimmedTo, type.create({
						...mark.attrs,
						...attributes
					}));
				});
				else tr.addMark(trimmedFrom, trimmedTo, type.create(attributes));
			});
		});
	}
	return canSetMark(state, tr, type);
};
var setMeta = (key, value) => ({ tr }) => {
	tr.setMeta(key, value);
	return true;
};
var setNode = (typeOrName, attributes = {}) => ({ state, dispatch, chain }) => {
	const type = getNodeType(typeOrName, state.schema);
	let attributesToCopy;
	if (state.selection.$anchor.sameParent(state.selection.$head)) attributesToCopy = state.selection.$anchor.parent.attrs;
	if (!type.isTextblock) {
		console.warn("[tiptap warn]: Currently \"setNode()\" only supports text block nodes.");
		return false;
	}
	return chain().command(({ commands }) => {
		if (setBlockType(type, {
			...attributesToCopy,
			...attributes
		})(state)) return true;
		return commands.clearNodes();
	}).command(({ state: updatedState }) => {
		return setBlockType(type, {
			...attributesToCopy,
			...attributes
		})(updatedState, dispatch);
	}).run();
};
var setNodeSelection = (position) => ({ tr, dispatch }) => {
	if (dispatch) {
		const { doc } = tr;
		const from = minMax(position, 0, doc.content.size);
		const selection = NodeSelection.create(doc, from);
		tr.setSelection(selection);
	}
	return true;
};
var setTextSelection = (position) => ({ tr, dispatch }) => {
	if (dispatch) {
		const { doc } = tr;
		const { from, to } = typeof position === "number" ? {
			from: position,
			to: position
		} : position;
		const minPos = TextSelection.atStart(doc).from;
		const maxPos = TextSelection.atEnd(doc).to;
		const resolvedFrom = minMax(from, minPos, maxPos);
		const resolvedEnd = minMax(to, minPos, maxPos);
		const selection = TextSelection.create(doc, resolvedFrom, resolvedEnd);
		tr.setSelection(selection);
	}
	return true;
};
var sinkListItem = (typeOrName) => ({ state, dispatch }) => {
	return sinkListItem$1(getNodeType(typeOrName, state.schema))(state, dispatch);
};
function ensureMarks(state, splittableMarks) {
	const marks = state.storedMarks || state.selection.$to.parentOffset && state.selection.$from.marks();
	if (marks) {
		const filteredMarks = marks.filter((mark) => splittableMarks === null || splittableMarks === void 0 ? void 0 : splittableMarks.includes(mark.type.name));
		state.tr.ensureMarks(filteredMarks);
	}
}
var splitBlock = ({ keepMarks = true } = {}) => ({ tr, state, dispatch, editor }) => {
	const { selection, doc } = tr;
	const { $from, $to } = selection;
	const extensionAttributes = editor.extensionManager.attributes;
	const newAttributes = getSplittedAttributes(extensionAttributes, $from.node().type.name, $from.node().attrs);
	if (selection instanceof NodeSelection && selection.node.isBlock) {
		if (!$from.parentOffset || !canSplit(doc, $from.pos)) return false;
		if (dispatch) {
			if (keepMarks) ensureMarks(state, editor.extensionManager.splittableMarks);
			tr.split($from.pos).scrollIntoView();
		}
		return true;
	}
	if (!$from.parent.isBlock) return false;
	const atEnd = $to.parentOffset === $to.parent.content.size;
	const deflt = $from.depth === 0 ? void 0 : defaultBlockAt($from.node(-1).contentMatchAt($from.indexAfter(-1)));
	let types = atEnd && deflt ? [{
		type: deflt,
		attrs: newAttributes
	}] : void 0;
	let can = canSplit(tr.doc, tr.mapping.map($from.pos), 1, types);
	if (!types && !can && canSplit(tr.doc, tr.mapping.map($from.pos), 1, deflt ? [{ type: deflt }] : void 0)) {
		can = true;
		types = deflt ? [{
			type: deflt,
			attrs: newAttributes
		}] : void 0;
	}
	if (dispatch) {
		if (can) {
			if (selection instanceof TextSelection) tr.deleteSelection();
			tr.split(tr.mapping.map($from.pos), 1, types);
			if (deflt && !atEnd && !$from.parentOffset && $from.parent.type !== deflt) {
				const first = tr.mapping.map($from.before());
				const $first = tr.doc.resolve(first);
				if ($from.node(-1).canReplaceWith($first.index(), $first.index() + 1, deflt)) tr.setNodeMarkup(tr.mapping.map($from.before()), deflt);
			}
		}
		if (keepMarks) ensureMarks(state, editor.extensionManager.splittableMarks);
		tr.scrollIntoView();
	}
	return can;
};
var splitListItem = (typeOrName, overrideAttrs = {}) => ({ tr, state, dispatch, editor }) => {
	var _a;
	const type = getNodeType(typeOrName, state.schema);
	const { $from, $to } = state.selection;
	const node = state.selection.node;
	if (node && node.isBlock || $from.depth < 2 || !$from.sameParent($to)) return false;
	const grandParent = $from.node(-1);
	if (grandParent.type !== type) return false;
	const extensionAttributes = editor.extensionManager.attributes;
	if ($from.parent.content.size === 0 && $from.node(-1).childCount === $from.indexAfter(-1)) {
		if ($from.depth === 2 || $from.node(-3).type !== type || $from.index(-2) !== $from.node(-2).childCount - 1) return false;
		if (dispatch) {
			let wrap = Fragment.empty;
			const depthBefore = $from.index(-1) ? 1 : $from.index(-2) ? 2 : 3;
			for (let d = $from.depth - depthBefore; d >= $from.depth - 3; d -= 1) wrap = Fragment.from($from.node(d).copy(wrap));
			const depthAfter = $from.indexAfter(-1) < $from.node(-2).childCount ? 1 : $from.indexAfter(-2) < $from.node(-3).childCount ? 2 : 3;
			const newNextTypeAttributes = {
				...getSplittedAttributes(extensionAttributes, $from.node().type.name, $from.node().attrs),
				...overrideAttrs
			};
			const nextType = ((_a = type.contentMatch.defaultType) === null || _a === void 0 ? void 0 : _a.createAndFill(newNextTypeAttributes)) || void 0;
			wrap = wrap.append(Fragment.from(type.createAndFill(null, nextType) || void 0));
			const start = $from.before($from.depth - (depthBefore - 1));
			tr.replace(start, $from.after(-depthAfter), new Slice(wrap, 4 - depthBefore, 0));
			let sel = -1;
			tr.doc.nodesBetween(start, tr.doc.content.size, (n, pos) => {
				if (sel > -1) return false;
				if (n.isTextblock && n.content.size === 0) sel = pos + 1;
			});
			if (sel > -1) tr.setSelection(TextSelection.near(tr.doc.resolve(sel)));
			tr.scrollIntoView();
		}
		return true;
	}
	const nextType = $to.pos === $from.end() ? grandParent.contentMatchAt(0).defaultType : null;
	const newTypeAttributes = {
		...getSplittedAttributes(extensionAttributes, grandParent.type.name, grandParent.attrs),
		...overrideAttrs
	};
	const newNextTypeAttributes = {
		...getSplittedAttributes(extensionAttributes, $from.node().type.name, $from.node().attrs),
		...overrideAttrs
	};
	tr.delete($from.pos, $to.pos);
	const types = nextType ? [{
		type,
		attrs: newTypeAttributes
	}, {
		type: nextType,
		attrs: newNextTypeAttributes
	}] : [{
		type,
		attrs: newTypeAttributes
	}];
	if (!canSplit(tr.doc, $from.pos, 2)) return false;
	if (dispatch) {
		const { selection, storedMarks } = state;
		const { splittableMarks } = editor.extensionManager;
		const marks = storedMarks || selection.$to.parentOffset && selection.$from.marks();
		tr.split($from.pos, 2, types).scrollIntoView();
		if (!marks || !dispatch) return true;
		const filteredMarks = marks.filter((mark) => splittableMarks.includes(mark.type.name));
		tr.ensureMarks(filteredMarks);
	}
	return true;
};
var joinListBackwards = (tr, listType) => {
	const list = findParentNode((node) => node.type === listType)(tr.selection);
	if (!list) return true;
	const before = tr.doc.resolve(Math.max(0, list.pos - 1)).before(list.depth);
	if (before === void 0) return true;
	const nodeBefore = tr.doc.nodeAt(before);
	if (!(list.node.type === (nodeBefore === null || nodeBefore === void 0 ? void 0 : nodeBefore.type) && canJoin(tr.doc, list.pos))) return true;
	tr.join(list.pos);
	return true;
};
var joinListForwards = (tr, listType) => {
	const list = findParentNode((node) => node.type === listType)(tr.selection);
	if (!list) return true;
	const after = tr.doc.resolve(list.start).after(list.depth);
	if (after === void 0) return true;
	const nodeAfter = tr.doc.nodeAt(after);
	if (!(list.node.type === (nodeAfter === null || nodeAfter === void 0 ? void 0 : nodeAfter.type) && canJoin(tr.doc, after))) return true;
	tr.join(after);
	return true;
};
var toggleList = (listTypeOrName, itemTypeOrName, keepMarks, attributes = {}) => ({ editor, tr, state, dispatch, chain, commands, can }) => {
	const { extensions, splittableMarks } = editor.extensionManager;
	const listType = getNodeType(listTypeOrName, state.schema);
	const itemType = getNodeType(itemTypeOrName, state.schema);
	const { selection, storedMarks } = state;
	const { $from, $to } = selection;
	const range = $from.blockRange($to);
	const marks = storedMarks || selection.$to.parentOffset && selection.$from.marks();
	if (!range) return false;
	const parentList = findParentNode((node) => isList(node.type.name, extensions))(selection);
	if (range.depth >= 1 && parentList && range.depth - parentList.depth <= 1) {
		if (parentList.node.type === listType) return commands.liftListItem(itemType);
		if (isList(parentList.node.type.name, extensions) && listType.validContent(parentList.node.content) && dispatch) return chain().command(() => {
			tr.setNodeMarkup(parentList.pos, listType);
			return true;
		}).command(() => joinListBackwards(tr, listType)).command(() => joinListForwards(tr, listType)).run();
	}
	if (!keepMarks || !marks || !dispatch) return chain().command(() => {
		if (can().wrapInList(listType, attributes)) return true;
		return commands.clearNodes();
	}).wrapInList(listType, attributes).command(() => joinListBackwards(tr, listType)).command(() => joinListForwards(tr, listType)).run();
	return chain().command(() => {
		const canWrapInList = can().wrapInList(listType, attributes);
		const filteredMarks = marks.filter((mark) => splittableMarks.includes(mark.type.name));
		tr.ensureMarks(filteredMarks);
		if (canWrapInList) return true;
		return commands.clearNodes();
	}).wrapInList(listType, attributes).command(() => joinListBackwards(tr, listType)).command(() => joinListForwards(tr, listType)).run();
};
var toggleMark = (typeOrName, attributes = {}, options = {}) => ({ state, commands }) => {
	const { extendEmptyMarkRange = false } = options;
	const type = getMarkType(typeOrName, state.schema);
	if (isMarkActive(state, type, attributes)) return commands.unsetMark(type, { extendEmptyMarkRange });
	return commands.setMark(type, attributes);
};
var toggleNode = (typeOrName, toggleTypeOrName, attributes = {}) => ({ state, commands }) => {
	const type = getNodeType(typeOrName, state.schema);
	const toggleType = getNodeType(toggleTypeOrName, state.schema);
	const isActive = isNodeActive(state, type, attributes);
	let attributesToCopy;
	if (state.selection.$anchor.sameParent(state.selection.$head)) attributesToCopy = state.selection.$anchor.parent.attrs;
	if (isActive) return commands.setNode(toggleType, attributesToCopy);
	return commands.setNode(type, {
		...attributesToCopy,
		...attributes
	});
};
var toggleWrap = (typeOrName, attributes = {}) => ({ state, commands }) => {
	const type = getNodeType(typeOrName, state.schema);
	if (isNodeActive(state, type, attributes)) return commands.lift(type);
	return commands.wrapIn(type, attributes);
};
var undoInputRule = () => ({ state, dispatch }) => {
	const plugins = state.plugins;
	for (let i = 0; i < plugins.length; i += 1) {
		const plugin = plugins[i];
		let undoable;
		if (plugin.spec.isInputRules && (undoable = plugin.getState(state))) {
			if (dispatch) {
				const tr = state.tr;
				const toUndo = undoable.transform;
				for (let j = toUndo.steps.length - 1; j >= 0; j -= 1) tr.step(toUndo.steps[j].invert(toUndo.docs[j]));
				if (undoable.text) {
					const marks = tr.doc.resolve(undoable.from).marks();
					tr.replaceWith(undoable.from, undoable.to, state.schema.text(undoable.text, marks));
				} else tr.delete(undoable.from, undoable.to);
			}
			return true;
		}
	}
	return false;
};
var unsetAllMarks = () => ({ tr, dispatch }) => {
	const { selection } = tr;
	const { empty, ranges } = selection;
	if (empty) return true;
	if (dispatch) ranges.forEach((range) => {
		tr.removeMark(range.$from.pos, range.$to.pos);
	});
	return true;
};
var unsetMark = (typeOrName, options = {}) => ({ tr, state, dispatch }) => {
	var _a;
	const { extendEmptyMarkRange = false } = options;
	const { selection } = tr;
	const type = getMarkType(typeOrName, state.schema);
	const { $from, empty, ranges } = selection;
	if (!dispatch) return true;
	if (empty && extendEmptyMarkRange) {
		let { from, to } = selection;
		const range = getMarkRange($from, type, (_a = $from.marks().find((mark) => mark.type === type)) === null || _a === void 0 ? void 0 : _a.attrs);
		if (range) {
			from = range.from;
			to = range.to;
		}
		tr.removeMark(from, to, type);
	} else ranges.forEach((range) => {
		tr.removeMark(range.$from.pos, range.$to.pos, type);
	});
	tr.removeStoredMark(type);
	return true;
};
var updateAttributes = (typeOrName, attributes = {}) => ({ tr, state, dispatch }) => {
	let nodeType = null;
	let markType = null;
	const schemaType = getSchemaTypeNameByName(typeof typeOrName === "string" ? typeOrName : typeOrName.name, state.schema);
	if (!schemaType) return false;
	if (schemaType === "node") nodeType = getNodeType(typeOrName, state.schema);
	if (schemaType === "mark") markType = getMarkType(typeOrName, state.schema);
	if (dispatch) tr.selection.ranges.forEach((range) => {
		const from = range.$from.pos;
		const to = range.$to.pos;
		let lastPos;
		let lastNode;
		let trimmedFrom;
		let trimmedTo;
		if (tr.selection.empty) state.doc.nodesBetween(from, to, (node, pos) => {
			if (nodeType && nodeType === node.type) {
				trimmedFrom = Math.max(pos, from);
				trimmedTo = Math.min(pos + node.nodeSize, to);
				lastPos = pos;
				lastNode = node;
			}
		});
		else state.doc.nodesBetween(from, to, (node, pos) => {
			if (pos < from && nodeType && nodeType === node.type) {
				trimmedFrom = Math.max(pos, from);
				trimmedTo = Math.min(pos + node.nodeSize, to);
				lastPos = pos;
				lastNode = node;
			}
			if (pos >= from && pos <= to) {
				if (nodeType && nodeType === node.type) tr.setNodeMarkup(pos, void 0, {
					...node.attrs,
					...attributes
				});
				if (markType && node.marks.length) node.marks.forEach((mark) => {
					if (markType === mark.type) {
						const trimmedFrom2 = Math.max(pos, from);
						const trimmedTo2 = Math.min(pos + node.nodeSize, to);
						tr.addMark(trimmedFrom2, trimmedTo2, markType.create({
							...mark.attrs,
							...attributes
						}));
					}
				});
			}
		});
		if (lastNode) {
			if (lastPos !== void 0) tr.setNodeMarkup(lastPos, void 0, {
				...lastNode.attrs,
				...attributes
			});
			if (markType && lastNode.marks.length) lastNode.marks.forEach((mark) => {
				if (markType === mark.type) tr.addMark(trimmedFrom, trimmedTo, markType.create({
					...mark.attrs,
					...attributes
				}));
			});
		}
	});
	return true;
};
var wrapIn = (typeOrName, attributes = {}) => ({ state, dispatch }) => {
	return wrapIn$1(getNodeType(typeOrName, state.schema), attributes)(state, dispatch);
};
var wrapInList = (typeOrName, attributes = {}) => ({ state, dispatch }) => {
	return wrapInList$1(getNodeType(typeOrName, state.schema), attributes)(state, dispatch);
};
var commands = /*#__PURE__*/ Object.freeze({
	__proto__: null,
	blur,
	clearContent,
	clearNodes,
	command,
	createParagraphNear,
	cut,
	deleteCurrentNode,
	deleteNode,
	deleteRange,
	deleteSelection,
	enter,
	exitCode,
	extendMarkRange,
	first,
	focus,
	forEach,
	insertContent,
	insertContentAt,
	joinBackward,
	joinDown,
	joinForward,
	joinItemBackward,
	joinItemForward,
	joinTextblockBackward,
	joinTextblockForward,
	joinUp,
	keyboardShortcut,
	lift,
	liftEmptyBlock,
	liftListItem,
	newlineInCode,
	resetAttributes,
	scrollIntoView,
	selectAll,
	selectNodeBackward,
	selectNodeForward,
	selectParentNode,
	selectTextblockEnd,
	selectTextblockStart,
	setContent,
	setMark,
	setMeta,
	setNode,
	setNodeSelection,
	setTextSelection,
	sinkListItem,
	splitBlock,
	splitListItem,
	toggleList,
	toggleMark,
	toggleNode,
	toggleWrap,
	undoInputRule,
	unsetAllMarks,
	unsetMark,
	updateAttributes,
	wrapIn,
	wrapInList
});
var Commands = Extension.create({
	name: "commands",
	addCommands() {
		return { ...commands };
	}
});
var Drop = Extension.create({
	name: "drop",
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("tiptapDrop"),
			props: { handleDrop: (_, e, slice, moved) => {
				this.editor.emit("drop", {
					editor: this.editor,
					event: e,
					slice,
					moved
				});
			} }
		})];
	}
});
var Editable = Extension.create({
	name: "editable",
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("editable"),
			props: { editable: () => this.editor.options.editable }
		})];
	}
});
var focusEventsPluginKey = new PluginKey("focusEvents");
var FocusEvents = Extension.create({
	name: "focusEvents",
	addProseMirrorPlugins() {
		const { editor } = this;
		return [new Plugin({
			key: focusEventsPluginKey,
			props: { handleDOMEvents: {
				focus: (view, event) => {
					editor.isFocused = true;
					const transaction = editor.state.tr.setMeta("focus", { event }).setMeta("addToHistory", false);
					view.dispatch(transaction);
					return false;
				},
				blur: (view, event) => {
					editor.isFocused = false;
					const transaction = editor.state.tr.setMeta("blur", { event }).setMeta("addToHistory", false);
					view.dispatch(transaction);
					return false;
				}
			} }
		})];
	}
});
var Keymap = Extension.create({
	name: "keymap",
	addKeyboardShortcuts() {
		const handleBackspace = () => this.editor.commands.first(({ commands }) => [
			() => commands.undoInputRule(),
			() => commands.command(({ tr }) => {
				const { selection, doc } = tr;
				const { empty, $anchor } = selection;
				const { pos, parent } = $anchor;
				const $parentPos = $anchor.parent.isTextblock && pos > 0 ? tr.doc.resolve(pos - 1) : $anchor;
				const parentIsIsolating = $parentPos.parent.type.spec.isolating;
				const parentPos = $anchor.pos - $anchor.parentOffset;
				const isAtStart = parentIsIsolating && $parentPos.parent.childCount === 1 ? parentPos === $anchor.pos : Selection.atStart(doc).from === pos;
				if (!empty || !parent.type.isTextblock || parent.textContent.length || !isAtStart || isAtStart && $anchor.parent.type.name === "paragraph") return false;
				return commands.clearNodes();
			}),
			() => commands.deleteSelection(),
			() => commands.joinBackward(),
			() => commands.selectNodeBackward()
		]);
		const handleDelete = () => this.editor.commands.first(({ commands }) => [
			() => commands.deleteSelection(),
			() => commands.deleteCurrentNode(),
			() => commands.joinForward(),
			() => commands.selectNodeForward()
		]);
		const handleEnter = () => this.editor.commands.first(({ commands }) => [
			() => commands.newlineInCode(),
			() => commands.createParagraphNear(),
			() => commands.liftEmptyBlock(),
			() => commands.splitBlock()
		]);
		const baseKeymap = {
			Enter: handleEnter,
			"Mod-Enter": () => this.editor.commands.exitCode(),
			Backspace: handleBackspace,
			"Mod-Backspace": handleBackspace,
			"Shift-Backspace": handleBackspace,
			Delete: handleDelete,
			"Mod-Delete": handleDelete,
			"Mod-a": () => this.editor.commands.selectAll()
		};
		const pcKeymap = { ...baseKeymap };
		const macKeymap = {
			...baseKeymap,
			"Ctrl-h": handleBackspace,
			"Alt-Backspace": handleBackspace,
			"Ctrl-d": handleDelete,
			"Ctrl-Alt-Backspace": handleDelete,
			"Alt-Delete": handleDelete,
			"Alt-d": handleDelete,
			"Ctrl-a": () => this.editor.commands.selectTextblockStart(),
			"Ctrl-e": () => this.editor.commands.selectTextblockEnd()
		};
		if (isiOS() || isMacOS()) return macKeymap;
		return pcKeymap;
	},
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("clearDocument"),
			appendTransaction: (transactions, oldState, newState) => {
				if (transactions.some((tr) => tr.getMeta("composition"))) return;
				const docChanges = transactions.some((transaction) => transaction.docChanged) && !oldState.doc.eq(newState.doc);
				const ignoreTr = transactions.some((transaction) => transaction.getMeta("preventClearDocument"));
				if (!docChanges || ignoreTr) return;
				const { empty, from, to } = oldState.selection;
				const allFrom = Selection.atStart(oldState.doc).from;
				const allEnd = Selection.atEnd(oldState.doc).to;
				if (empty || !(from === allFrom && to === allEnd)) return;
				if (!isNodeEmpty(newState.doc)) return;
				const tr = newState.tr;
				const state = createChainableState({
					state: newState,
					transaction: tr
				});
				const { commands } = new CommandManager({
					editor: this.editor,
					state
				});
				commands.clearNodes();
				if (!tr.steps.length) return;
				return tr;
			}
		})];
	}
});
var Paste = Extension.create({
	name: "paste",
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("tiptapPaste"),
			props: { handlePaste: (_view, e, slice) => {
				this.editor.emit("paste", {
					editor: this.editor,
					event: e,
					slice
				});
			} }
		})];
	}
});
var Tabindex = Extension.create({
	name: "tabindex",
	addProseMirrorPlugins() {
		return [new Plugin({
			key: new PluginKey("tabindex"),
			props: { attributes: () => this.editor.isEditable ? { tabindex: "0" } : {} }
		})];
	}
});
var index = /*#__PURE__*/ Object.freeze({
	__proto__: null,
	ClipboardTextSerializer,
	Commands,
	Drop,
	Editable,
	FocusEvents,
	Keymap,
	Paste,
	Tabindex,
	focusEventsPluginKey
});
var NodePos = class NodePos {
	get name() {
		return this.node.type.name;
	}
	constructor(pos, editor, isBlock = false, node = null) {
		this.currentNode = null;
		this.actualDepth = null;
		this.isBlock = isBlock;
		this.resolvedPos = pos;
		this.editor = editor;
		this.currentNode = node;
	}
	get node() {
		return this.currentNode || this.resolvedPos.node();
	}
	get element() {
		return this.editor.view.domAtPos(this.pos).node;
	}
	get depth() {
		var _a;
		return (_a = this.actualDepth) !== null && _a !== void 0 ? _a : this.resolvedPos.depth;
	}
	get pos() {
		return this.resolvedPos.pos;
	}
	get content() {
		return this.node.content;
	}
	set content(content) {
		let from = this.from;
		let to = this.to;
		if (this.isBlock) {
			if (this.content.size === 0) {
				console.error(`You can’t set content on a block node. Tried to set content on ${this.name} at ${this.pos}`);
				return;
			}
			from = this.from + 1;
			to = this.to - 1;
		}
		this.editor.commands.insertContentAt({
			from,
			to
		}, content);
	}
	get attributes() {
		return this.node.attrs;
	}
	get textContent() {
		return this.node.textContent;
	}
	get size() {
		return this.node.nodeSize;
	}
	get from() {
		if (this.isBlock) return this.pos;
		return this.resolvedPos.start(this.resolvedPos.depth);
	}
	get range() {
		return {
			from: this.from,
			to: this.to
		};
	}
	get to() {
		if (this.isBlock) return this.pos + this.size;
		return this.resolvedPos.end(this.resolvedPos.depth) + (this.node.isText ? 0 : 1);
	}
	get parent() {
		if (this.depth === 0) return null;
		const parentPos = this.resolvedPos.start(this.resolvedPos.depth - 1);
		const $pos = this.resolvedPos.doc.resolve(parentPos);
		return new NodePos($pos, this.editor);
	}
	get before() {
		let $pos = this.resolvedPos.doc.resolve(this.from - (this.isBlock ? 1 : 2));
		if ($pos.depth !== this.depth) $pos = this.resolvedPos.doc.resolve(this.from - 3);
		return new NodePos($pos, this.editor);
	}
	get after() {
		let $pos = this.resolvedPos.doc.resolve(this.to + (this.isBlock ? 2 : 1));
		if ($pos.depth !== this.depth) $pos = this.resolvedPos.doc.resolve(this.to + 3);
		return new NodePos($pos, this.editor);
	}
	get children() {
		const children = [];
		this.node.content.forEach((node, offset) => {
			const isBlock = node.isBlock && !node.isTextblock;
			const isNonTextAtom = node.isAtom && !node.isText;
			const targetPos = this.pos + offset + (isNonTextAtom ? 0 : 1);
			if (targetPos < 0 || targetPos > this.resolvedPos.doc.nodeSize - 2) return;
			const $pos = this.resolvedPos.doc.resolve(targetPos);
			if (!isBlock && $pos.depth <= this.depth) return;
			const childNodePos = new NodePos($pos, this.editor, isBlock, isBlock ? node : null);
			if (isBlock) childNodePos.actualDepth = this.depth + 1;
			children.push(new NodePos($pos, this.editor, isBlock, isBlock ? node : null));
		});
		return children;
	}
	get firstChild() {
		return this.children[0] || null;
	}
	get lastChild() {
		const children = this.children;
		return children[children.length - 1] || null;
	}
	closest(selector, attributes = {}) {
		let node = null;
		let currentNode = this.parent;
		while (currentNode && !node) {
			if (currentNode.node.type.name === selector) {
				if (Object.keys(attributes).length > 0) {
					const nodeAttributes = currentNode.node.attrs;
					const attrKeys = Object.keys(attributes);
					for (let index = 0; index < attrKeys.length; index += 1) {
						const key = attrKeys[index];
						if (nodeAttributes[key] !== attributes[key]) break;
					}
				} else node = currentNode;
			}
			currentNode = currentNode.parent;
		}
		return node;
	}
	querySelector(selector, attributes = {}) {
		return this.querySelectorAll(selector, attributes, true)[0] || null;
	}
	querySelectorAll(selector, attributes = {}, firstItemOnly = false) {
		let nodes = [];
		if (!this.children || this.children.length === 0) return nodes;
		const attrKeys = Object.keys(attributes);
		/**
		* Finds all children recursively that match the selector and attributes
		* If firstItemOnly is true, it will return the first item found
		*/
		this.children.forEach((childPos) => {
			if (firstItemOnly && nodes.length > 0) return;
			if (childPos.node.type.name === selector) {
				if (attrKeys.every((key) => attributes[key] === childPos.node.attrs[key])) nodes.push(childPos);
			}
			if (firstItemOnly && nodes.length > 0) return;
			nodes = nodes.concat(childPos.querySelectorAll(selector, attributes, firstItemOnly));
		});
		return nodes;
	}
	setAttribute(attributes) {
		const { tr } = this.editor.state;
		tr.setNodeMarkup(this.from, void 0, {
			...this.node.attrs,
			...attributes
		});
		this.editor.view.dispatch(tr);
	}
};
var style = `.ProseMirror {
  position: relative;
}

.ProseMirror {
  word-wrap: break-word;
  white-space: pre-wrap;
  white-space: break-spaces;
  -webkit-font-variant-ligatures: none;
  font-variant-ligatures: none;
  font-feature-settings: "liga" 0; /* the above doesn't seem to work in Edge */
}

.ProseMirror [contenteditable="false"] {
  white-space: normal;
}

.ProseMirror [contenteditable="false"] [contenteditable="true"] {
  white-space: pre-wrap;
}

.ProseMirror pre {
  white-space: pre-wrap;
}

img.ProseMirror-separator {
  display: inline !important;
  border: none !important;
  margin: 0 !important;
  width: 0 !important;
  height: 0 !important;
}

.ProseMirror-gapcursor {
  display: none;
  pointer-events: none;
  position: absolute;
  margin: 0;
}

.ProseMirror-gapcursor:after {
  content: "";
  display: block;
  position: absolute;
  top: -2px;
  width: 20px;
  border-top: 1px solid black;
  animation: ProseMirror-cursor-blink 1.1s steps(2, start) infinite;
}

@keyframes ProseMirror-cursor-blink {
  to {
    visibility: hidden;
  }
}

.ProseMirror-hideselection *::selection {
  background: transparent;
}

.ProseMirror-hideselection *::-moz-selection {
  background: transparent;
}

.ProseMirror-hideselection * {
  caret-color: transparent;
}

.ProseMirror-focused .ProseMirror-gapcursor {
  display: block;
}

.tippy-box[data-animation=fade][data-state=hidden] {
  opacity: 0
}`;
function createStyleTag(style, nonce, suffix) {
	const tiptapStyleTag = document.querySelector(`style[data-tiptap-style${suffix ? `-${suffix}` : ""}]`);
	if (tiptapStyleTag !== null) return tiptapStyleTag;
	const styleNode = document.createElement("style");
	if (nonce) styleNode.setAttribute("nonce", nonce);
	styleNode.setAttribute(`data-tiptap-style${suffix ? `-${suffix}` : ""}`, "");
	styleNode.innerHTML = style;
	document.getElementsByTagName("head")[0].appendChild(styleNode);
	return styleNode;
}
var Editor = class extends EventEmitter {
	constructor(options = {}) {
		super();
		this.isFocused = false;
		/**
		* The editor is considered initialized after the `create` event has been emitted.
		*/
		this.isInitialized = false;
		this.extensionStorage = {};
		this.options = {
			element: document.createElement("div"),
			content: "",
			injectCSS: true,
			injectNonce: void 0,
			extensions: [],
			autofocus: false,
			editable: true,
			editorProps: {},
			parseOptions: {},
			coreExtensionOptions: {},
			enableInputRules: true,
			enablePasteRules: true,
			enableCoreExtensions: true,
			enableContentCheck: false,
			emitContentError: false,
			onBeforeCreate: () => null,
			onCreate: () => null,
			onUpdate: () => null,
			onSelectionUpdate: () => null,
			onTransaction: () => null,
			onFocus: () => null,
			onBlur: () => null,
			onDestroy: () => null,
			onContentError: ({ error }) => {
				throw error;
			},
			onPaste: () => null,
			onDrop: () => null
		};
		this.isCapturingTransaction = false;
		this.capturedTransaction = null;
		this.setOptions(options);
		this.createExtensionManager();
		this.createCommandManager();
		this.createSchema();
		this.on("beforeCreate", this.options.onBeforeCreate);
		this.emit("beforeCreate", { editor: this });
		this.on("contentError", this.options.onContentError);
		this.createView();
		this.injectCSS();
		this.on("create", this.options.onCreate);
		this.on("update", this.options.onUpdate);
		this.on("selectionUpdate", this.options.onSelectionUpdate);
		this.on("transaction", this.options.onTransaction);
		this.on("focus", this.options.onFocus);
		this.on("blur", this.options.onBlur);
		this.on("destroy", this.options.onDestroy);
		this.on("drop", ({ event, slice, moved }) => this.options.onDrop(event, slice, moved));
		this.on("paste", ({ event, slice }) => this.options.onPaste(event, slice));
		window.setTimeout(() => {
			if (this.isDestroyed) return;
			this.commands.focus(this.options.autofocus);
			this.emit("create", { editor: this });
			this.isInitialized = true;
		}, 0);
	}
	/**
	* Returns the editor storage.
	*/
	get storage() {
		return this.extensionStorage;
	}
	/**
	* An object of all registered commands.
	*/
	get commands() {
		return this.commandManager.commands;
	}
	/**
	* Create a command chain to call multiple commands at once.
	*/
	chain() {
		return this.commandManager.chain();
	}
	/**
	* Check if a command or a command chain can be executed. Without executing it.
	*/
	can() {
		return this.commandManager.can();
	}
	/**
	* Inject CSS styles.
	*/
	injectCSS() {
		if (this.options.injectCSS && document) this.css = createStyleTag(style, this.options.injectNonce);
	}
	/**
	* Update editor options.
	*
	* @param options A list of options
	*/
	setOptions(options = {}) {
		this.options = {
			...this.options,
			...options
		};
		if (!this.view || !this.state || this.isDestroyed) return;
		if (this.options.editorProps) this.view.setProps(this.options.editorProps);
		this.view.updateState(this.state);
	}
	/**
	* Update editable state of the editor.
	*/
	setEditable(editable, emitUpdate = true) {
		this.setOptions({ editable });
		if (emitUpdate) this.emit("update", {
			editor: this,
			transaction: this.state.tr
		});
	}
	/**
	* Returns whether the editor is editable.
	*/
	get isEditable() {
		return this.options.editable && this.view && this.view.editable;
	}
	/**
	* Returns the editor state.
	*/
	get state() {
		return this.view.state;
	}
	/**
	* Register a ProseMirror plugin.
	*
	* @param plugin A ProseMirror plugin
	* @param handlePlugins Control how to merge the plugin into the existing plugins.
	* @returns The new editor state
	*/
	registerPlugin(plugin, handlePlugins) {
		const plugins = isFunction(handlePlugins) ? handlePlugins(plugin, [...this.state.plugins]) : [...this.state.plugins, plugin];
		const state = this.state.reconfigure({ plugins });
		this.view.updateState(state);
		return state;
	}
	/**
	* Unregister a ProseMirror plugin.
	*
	* @param nameOrPluginKeyToRemove The plugins name
	* @returns The new editor state or undefined if the editor is destroyed
	*/
	unregisterPlugin(nameOrPluginKeyToRemove) {
		if (this.isDestroyed) return;
		const prevPlugins = this.state.plugins;
		let plugins = prevPlugins;
		[].concat(nameOrPluginKeyToRemove).forEach((nameOrPluginKey) => {
			const name = typeof nameOrPluginKey === "string" ? `${nameOrPluginKey}$` : nameOrPluginKey.key;
			plugins = plugins.filter((plugin) => !plugin.key.startsWith(name));
		});
		if (prevPlugins.length === plugins.length) return;
		const state = this.state.reconfigure({ plugins });
		this.view.updateState(state);
		return state;
	}
	/**
	* Creates an extension manager.
	*/
	createExtensionManager() {
		var _a, _b;
		const allExtensions = [...this.options.enableCoreExtensions ? [
			Editable,
			ClipboardTextSerializer.configure({ blockSeparator: (_b = (_a = this.options.coreExtensionOptions) === null || _a === void 0 ? void 0 : _a.clipboardTextSerializer) === null || _b === void 0 ? void 0 : _b.blockSeparator }),
			Commands,
			FocusEvents,
			Keymap,
			Tabindex,
			Drop,
			Paste
		].filter((ext) => {
			if (typeof this.options.enableCoreExtensions === "object") return this.options.enableCoreExtensions[ext.name] !== false;
			return true;
		}) : [], ...this.options.extensions].filter((extension) => {
			return [
				"extension",
				"node",
				"mark"
			].includes(extension === null || extension === void 0 ? void 0 : extension.type);
		});
		this.extensionManager = new ExtensionManager(allExtensions, this);
	}
	/**
	* Creates an command manager.
	*/
	createCommandManager() {
		this.commandManager = new CommandManager({ editor: this });
	}
	/**
	* Creates a ProseMirror schema.
	*/
	createSchema() {
		this.schema = this.extensionManager.schema;
	}
	/**
	* Creates a ProseMirror view.
	*/
	createView() {
		var _a;
		let doc;
		try {
			doc = createDocument(this.options.content, this.schema, this.options.parseOptions, { errorOnInvalidContent: this.options.enableContentCheck });
		} catch (e) {
			if (!(e instanceof Error) || !["[tiptap error]: Invalid JSON content", "[tiptap error]: Invalid HTML content"].includes(e.message)) throw e;
			this.emit("contentError", {
				editor: this,
				error: e,
				disableCollaboration: () => {
					if (this.storage.collaboration) this.storage.collaboration.isDisabled = true;
					this.options.extensions = this.options.extensions.filter((extension) => extension.name !== "collaboration");
					this.createExtensionManager();
				}
			});
			doc = createDocument(this.options.content, this.schema, this.options.parseOptions, { errorOnInvalidContent: false });
		}
		const selection = resolveFocusPosition(doc, this.options.autofocus);
		this.view = new EditorView(this.options.element, {
			...this.options.editorProps,
			attributes: {
				role: "textbox",
				...(_a = this.options.editorProps) === null || _a === void 0 ? void 0 : _a.attributes
			},
			dispatchTransaction: this.dispatchTransaction.bind(this),
			state: EditorState.create({
				doc,
				selection: selection || void 0
			})
		});
		const newState = this.state.reconfigure({ plugins: this.extensionManager.plugins });
		this.view.updateState(newState);
		this.createNodeViews();
		this.prependClass();
		const dom = this.view.dom;
		dom.editor = this;
	}
	/**
	* Creates all node views.
	*/
	createNodeViews() {
		if (this.view.isDestroyed) return;
		this.view.setProps({ nodeViews: this.extensionManager.nodeViews });
	}
	/**
	* Prepend class name to element.
	*/
	prependClass() {
		this.view.dom.className = `tiptap ${this.view.dom.className}`;
	}
	captureTransaction(fn) {
		this.isCapturingTransaction = true;
		fn();
		this.isCapturingTransaction = false;
		const tr = this.capturedTransaction;
		this.capturedTransaction = null;
		return tr;
	}
	/**
	* The callback over which to send transactions (state updates) produced by the view.
	*
	* @param transaction An editor state transaction
	*/
	dispatchTransaction(transaction) {
		if (this.view.isDestroyed) return;
		if (this.isCapturingTransaction) {
			if (!this.capturedTransaction) {
				this.capturedTransaction = transaction;
				return;
			}
			transaction.steps.forEach((step) => {
				var _a;
				return (_a = this.capturedTransaction) === null || _a === void 0 ? void 0 : _a.step(step);
			});
			return;
		}
		const state = this.state.apply(transaction);
		const selectionHasChanged = !this.state.selection.eq(state.selection);
		this.emit("beforeTransaction", {
			editor: this,
			transaction,
			nextState: state
		});
		this.view.updateState(state);
		this.emit("transaction", {
			editor: this,
			transaction
		});
		if (selectionHasChanged) this.emit("selectionUpdate", {
			editor: this,
			transaction
		});
		const focus = transaction.getMeta("focus");
		const blur = transaction.getMeta("blur");
		if (focus) this.emit("focus", {
			editor: this,
			event: focus.event,
			transaction
		});
		if (blur) this.emit("blur", {
			editor: this,
			event: blur.event,
			transaction
		});
		if (!transaction.docChanged || transaction.getMeta("preventUpdate")) return;
		this.emit("update", {
			editor: this,
			transaction
		});
	}
	/**
	* Get attributes of the currently selected node or mark.
	*/
	getAttributes(nameOrType) {
		return getAttributes(this.state, nameOrType);
	}
	isActive(nameOrAttributes, attributesOrUndefined) {
		const name = typeof nameOrAttributes === "string" ? nameOrAttributes : null;
		const attributes = typeof nameOrAttributes === "string" ? attributesOrUndefined : nameOrAttributes;
		return isActive(this.state, name, attributes);
	}
	/**
	* Get the document as JSON.
	*/
	getJSON() {
		return this.state.doc.toJSON();
	}
	/**
	* Get the document as HTML.
	*/
	getHTML() {
		return getHTMLFromFragment(this.state.doc.content, this.schema);
	}
	/**
	* Get the document as text.
	*/
	getText(options) {
		const { blockSeparator = "\n\n", textSerializers = {} } = options || {};
		return getText(this.state.doc, {
			blockSeparator,
			textSerializers: {
				...getTextSerializersFromSchema(this.schema),
				...textSerializers
			}
		});
	}
	/**
	* Check if there is no content.
	*/
	get isEmpty() {
		return isNodeEmpty(this.state.doc);
	}
	/**
	* Get the number of characters for the current document.
	*
	* @deprecated
	*/
	getCharacterCount() {
		console.warn("[tiptap warn]: \"editor.getCharacterCount()\" is deprecated. Please use \"editor.storage.characterCount.characters()\" instead.");
		return this.state.doc.content.size - 2;
	}
	/**
	* Destroy the editor.
	*/
	destroy() {
		this.emit("destroy");
		if (this.view) {
			const dom = this.view.dom;
			if (dom && dom.editor) delete dom.editor;
			this.view.destroy();
		}
		this.removeAllListeners();
	}
	/**
	* Check if the editor is already destroyed.
	*/
	get isDestroyed() {
		var _a;
		return !((_a = this.view) === null || _a === void 0 ? void 0 : _a.docView);
	}
	$node(selector, attributes) {
		var _a;
		return ((_a = this.$doc) === null || _a === void 0 ? void 0 : _a.querySelector(selector, attributes)) || null;
	}
	$nodes(selector, attributes) {
		var _a;
		return ((_a = this.$doc) === null || _a === void 0 ? void 0 : _a.querySelectorAll(selector, attributes)) || null;
	}
	$pos(pos) {
		return new NodePos(this.state.doc.resolve(pos), this);
	}
	get $doc() {
		return this.$pos(0);
	}
};
/**
* Build an input rule that adds a mark when the
* matched text is typed into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#input-rules
*/
function markInputRule(config) {
	return new InputRule({
		find: config.find,
		handler: ({ state, range, match }) => {
			const attributes = callOrReturn(config.getAttributes, void 0, match);
			if (attributes === false || attributes === null) return null;
			const { tr } = state;
			const captureGroup = match[match.length - 1];
			const fullMatch = match[0];
			if (captureGroup) {
				const startSpaces = fullMatch.search(/\S/);
				const textStart = range.from + fullMatch.indexOf(captureGroup);
				const textEnd = textStart + captureGroup.length;
				if (getMarksBetween(range.from, range.to, state.doc).filter((item) => {
					return item.mark.type.excluded.find((type) => type === config.type && type !== item.mark.type);
				}).filter((item) => item.to > textStart).length) return null;
				if (textEnd < range.to) tr.delete(textEnd, range.to);
				if (textStart > range.from) tr.delete(range.from + startSpaces, textStart);
				const markEnd = range.from + startSpaces + captureGroup.length;
				tr.addMark(range.from + startSpaces, markEnd, config.type.create(attributes || {}));
				tr.removeStoredMark(config.type);
			}
		}
	});
}
/**
* Build an input rule that adds a node when the
* matched text is typed into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#input-rules
*/
function nodeInputRule(config) {
	return new InputRule({
		find: config.find,
		handler: ({ state, range, match }) => {
			const attributes = callOrReturn(config.getAttributes, void 0, match) || {};
			const { tr } = state;
			const start = range.from;
			let end = range.to;
			const newNode = config.type.create(attributes);
			if (match[1]) {
				let matchStart = start + match[0].lastIndexOf(match[1]);
				if (matchStart > end) matchStart = end;
				else end = matchStart + match[1].length;
				const lastChar = match[0][match[0].length - 1];
				tr.insertText(lastChar, start + match[0].length - 1);
				tr.replaceWith(matchStart, end, newNode);
			} else if (match[0]) {
				const insertionStart = config.type.isInline ? start : start - 1;
				tr.insert(insertionStart, config.type.create(attributes)).delete(tr.mapping.map(start), tr.mapping.map(end));
			}
			tr.scrollIntoView();
		}
	});
}
/**
* Build an input rule that changes the type of a textblock when the
* matched text is typed into it. When using a regular expresion you’ll
* probably want the regexp to start with `^`, so that the pattern can
* only occur at the start of a textblock.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#input-rules
*/
function textblockTypeInputRule(config) {
	return new InputRule({
		find: config.find,
		handler: ({ state, range, match }) => {
			const $start = state.doc.resolve(range.from);
			const attributes = callOrReturn(config.getAttributes, void 0, match) || {};
			if (!$start.node(-1).canReplaceWith($start.index(-1), $start.indexAfter(-1), config.type)) return null;
			state.tr.delete(range.from, range.to).setBlockType(range.from, range.from, config.type, attributes);
		}
	});
}
/**
* Build an input rule that replaces text when the
* matched text is typed into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#input-rules
*/
function textInputRule(config) {
	return new InputRule({
		find: config.find,
		handler: ({ state, range, match }) => {
			let insert = config.replace;
			let start = range.from;
			const end = range.to;
			if (match[1]) {
				const offset = match[0].lastIndexOf(match[1]);
				insert += match[0].slice(offset + match[1].length);
				start += offset;
				const cutOff = start - end;
				if (cutOff > 0) {
					insert = match[0].slice(offset - cutOff, offset) + insert;
					start = end;
				}
			}
			state.tr.insertText(insert, start, end);
		}
	});
}
/**
* Build an input rule for automatically wrapping a textblock when a
* given string is typed. When using a regular expresion you’ll
* probably want the regexp to start with `^`, so that the pattern can
* only occur at the start of a textblock.
*
* `type` is the type of node to wrap in.
*
* By default, if there’s a node with the same type above the newly
* wrapped node, the rule will try to join those
* two nodes. You can pass a join predicate, which takes a regular
* expression match and the node before the wrapped node, and can
* return a boolean to indicate whether a join should happen.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#input-rules
*/
function wrappingInputRule(config) {
	return new InputRule({
		find: config.find,
		handler: ({ state, range, match, chain }) => {
			const attributes = callOrReturn(config.getAttributes, void 0, match) || {};
			const tr = state.tr.delete(range.from, range.to);
			const blockRange = tr.doc.resolve(range.from).blockRange();
			const wrapping = blockRange && findWrapping(blockRange, config.type, attributes);
			if (!wrapping) return null;
			tr.wrap(blockRange, wrapping);
			if (config.keepMarks && config.editor) {
				const { selection, storedMarks } = state;
				const { splittableMarks } = config.editor.extensionManager;
				const marks = storedMarks || selection.$to.parentOffset && selection.$from.marks();
				if (marks) {
					const filteredMarks = marks.filter((mark) => splittableMarks.includes(mark.type.name));
					tr.ensureMarks(filteredMarks);
				}
			}
			if (config.keepAttributes) {
				/** If the nodeType is `bulletList` or `orderedList` set the `nodeType` as `listItem` */
				const nodeType = config.type.name === "bulletList" || config.type.name === "orderedList" ? "listItem" : "taskList";
				chain().updateAttributes(nodeType, attributes).run();
			}
			const before = tr.doc.resolve(range.from - 1).nodeBefore;
			if (before && before.type === config.type && canJoin(tr.doc, range.from - 1) && (!config.joinPredicate || config.joinPredicate(match, before))) tr.join(range.from - 1);
		}
	});
}
/**
* The Node class is used to create custom node extensions.
* @see https://tiptap.dev/api/extensions#create-a-new-extension
*/
var Node = class Node {
	constructor(config = {}) {
		this.type = "node";
		this.name = "node";
		this.parent = null;
		this.child = null;
		this.config = {
			name: this.name,
			defaultOptions: {}
		};
		this.config = {
			...this.config,
			...config
		};
		this.name = this.config.name;
		if (config.defaultOptions && Object.keys(config.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${this.name}".`);
		this.options = this.config.defaultOptions;
		if (this.config.addOptions) this.options = callOrReturn(getExtensionField(this, "addOptions", { name: this.name }));
		this.storage = callOrReturn(getExtensionField(this, "addStorage", {
			name: this.name,
			options: this.options
		})) || {};
	}
	static create(config = {}) {
		return new Node(config);
	}
	configure(options = {}) {
		const extension = this.extend({
			...this.config,
			addOptions: () => {
				return mergeDeep(this.options, options);
			}
		});
		extension.name = this.name;
		extension.parent = this.parent;
		return extension;
	}
	extend(extendedConfig = {}) {
		const extension = new Node(extendedConfig);
		extension.parent = this;
		this.child = extension;
		extension.name = extendedConfig.name ? extendedConfig.name : extension.parent.name;
		if (extendedConfig.defaultOptions && Object.keys(extendedConfig.defaultOptions).length > 0) console.warn(`[tiptap warn]: BREAKING CHANGE: "defaultOptions" is deprecated. Please use "addOptions" instead. Found in extension: "${extension.name}".`);
		extension.options = callOrReturn(getExtensionField(extension, "addOptions", { name: extension.name }));
		extension.storage = callOrReturn(getExtensionField(extension, "addStorage", {
			name: extension.name,
			options: extension.options
		}));
		return extension;
	}
};
/**
* Node views are used to customize the rendered DOM structure of a node.
* @see https://tiptap.dev/guide/node-views
*/
var NodeView = class {
	constructor(component, props, options) {
		this.isDragging = false;
		this.component = component;
		this.editor = props.editor;
		this.options = {
			stopEvent: null,
			ignoreMutation: null,
			...options
		};
		this.extension = props.extension;
		this.node = props.node;
		this.decorations = props.decorations;
		this.innerDecorations = props.innerDecorations;
		this.view = props.view;
		this.HTMLAttributes = props.HTMLAttributes;
		this.getPos = props.getPos;
		this.mount();
	}
	mount() {}
	get dom() {
		return this.editor.view.dom;
	}
	get contentDOM() {
		return null;
	}
	onDragStart(event) {
		var _a, _b, _c, _d, _e, _f, _g;
		const { view } = this.editor;
		const target = event.target;
		const dragHandle = target.nodeType === 3 ? (_a = target.parentElement) === null || _a === void 0 ? void 0 : _a.closest("[data-drag-handle]") : target.closest("[data-drag-handle]");
		if (!this.dom || ((_b = this.contentDOM) === null || _b === void 0 ? void 0 : _b.contains(target)) || !dragHandle) return;
		let x = 0;
		let y = 0;
		if (this.dom !== dragHandle) {
			const domBox = this.dom.getBoundingClientRect();
			const handleBox = dragHandle.getBoundingClientRect();
			const offsetX = (_c = event.offsetX) !== null && _c !== void 0 ? _c : (_d = event.nativeEvent) === null || _d === void 0 ? void 0 : _d.offsetX;
			const offsetY = (_e = event.offsetY) !== null && _e !== void 0 ? _e : (_f = event.nativeEvent) === null || _f === void 0 ? void 0 : _f.offsetY;
			x = handleBox.x - domBox.x + offsetX;
			y = handleBox.y - domBox.y + offsetY;
		}
		const clonedNode = this.dom.cloneNode(true);
		(_g = event.dataTransfer) === null || _g === void 0 || _g.setDragImage(clonedNode, x, y);
		const pos = this.getPos();
		if (typeof pos !== "number") return;
		const selection = NodeSelection.create(view.state.doc, pos);
		const transaction = view.state.tr.setSelection(selection);
		view.dispatch(transaction);
	}
	stopEvent(event) {
		var _a;
		if (!this.dom) return false;
		if (typeof this.options.stopEvent === "function") return this.options.stopEvent({ event });
		const target = event.target;
		if (!(this.dom.contains(target) && !((_a = this.contentDOM) === null || _a === void 0 ? void 0 : _a.contains(target)))) return false;
		const isDragEvent = event.type.startsWith("drag");
		const isDropEvent = event.type === "drop";
		if (([
			"INPUT",
			"BUTTON",
			"SELECT",
			"TEXTAREA"
		].includes(target.tagName) || target.isContentEditable) && !isDropEvent && !isDragEvent) return true;
		const { isEditable } = this.editor;
		const { isDragging } = this;
		const isDraggable = !!this.node.type.spec.draggable;
		const isSelectable = NodeSelection.isSelectable(this.node);
		const isCopyEvent = event.type === "copy";
		const isPasteEvent = event.type === "paste";
		const isCutEvent = event.type === "cut";
		const isClickEvent = event.type === "mousedown";
		if (!isDraggable && isSelectable && isDragEvent && event.target === this.dom) event.preventDefault();
		if (isDraggable && isDragEvent && !isDragging && event.target === this.dom) {
			event.preventDefault();
			return false;
		}
		if (isDraggable && isEditable && !isDragging && isClickEvent) {
			const dragHandle = target.closest("[data-drag-handle]");
			if (dragHandle && (this.dom === dragHandle || this.dom.contains(dragHandle))) {
				this.isDragging = true;
				document.addEventListener("dragend", () => {
					this.isDragging = false;
				}, { once: true });
				document.addEventListener("drop", () => {
					this.isDragging = false;
				}, { once: true });
				document.addEventListener("mouseup", () => {
					this.isDragging = false;
				}, { once: true });
			}
		}
		if (isDragging || isDropEvent || isCopyEvent || isPasteEvent || isCutEvent || isClickEvent && isSelectable) return false;
		return true;
	}
	/**
	* Called when a DOM [mutation](https://developer.mozilla.org/en-US/docs/Web/API/MutationObserver) or a selection change happens within the view.
	* @return `false` if the editor should re-read the selection or re-parse the range around the mutation
	* @return `true` if it can safely be ignored.
	*/
	ignoreMutation(mutation) {
		if (!this.dom || !this.contentDOM) return true;
		if (typeof this.options.ignoreMutation === "function") return this.options.ignoreMutation({ mutation });
		if (this.node.isLeaf || this.node.isAtom) return true;
		if (mutation.type === "selection") return false;
		if (this.dom.contains(mutation.target) && mutation.type === "childList" && (isiOS() || isAndroid()) && this.editor.isFocused) {
			if ([...Array.from(mutation.addedNodes), ...Array.from(mutation.removedNodes)].every((node) => node.isContentEditable)) return false;
		}
		if (this.contentDOM === mutation.target && mutation.type === "attributes") return true;
		if (this.contentDOM.contains(mutation.target)) return false;
		return true;
	}
	/**
	* Update the attributes of the prosemirror node.
	*/
	updateAttributes(attributes) {
		this.editor.commands.command(({ tr }) => {
			const pos = this.getPos();
			if (typeof pos !== "number") return false;
			tr.setNodeMarkup(pos, void 0, {
				...this.node.attrs,
				...attributes
			});
			return true;
		});
	}
	/**
	* Delete the node.
	*/
	deleteNode() {
		const from = this.getPos();
		if (typeof from !== "number") return;
		const to = from + this.node.nodeSize;
		this.editor.commands.deleteRange({
			from,
			to
		});
	}
};
/**
* Build an paste rule that adds a mark when the
* matched text is pasted into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#paste-rules
*/
function markPasteRule(config) {
	return new PasteRule({
		find: config.find,
		handler: ({ state, range, match, pasteEvent }) => {
			const attributes = callOrReturn(config.getAttributes, void 0, match, pasteEvent);
			if (attributes === false || attributes === null) return null;
			const { tr } = state;
			const captureGroup = match[match.length - 1];
			const fullMatch = match[0];
			let markEnd = range.to;
			if (captureGroup) {
				const startSpaces = fullMatch.search(/\S/);
				const textStart = range.from + fullMatch.indexOf(captureGroup);
				const textEnd = textStart + captureGroup.length;
				if (getMarksBetween(range.from, range.to, state.doc).filter((item) => {
					return item.mark.type.excluded.find((type) => type === config.type && type !== item.mark.type);
				}).filter((item) => item.to > textStart).length) return null;
				if (textEnd < range.to) tr.delete(textEnd, range.to);
				if (textStart > range.from) tr.delete(range.from + startSpaces, textStart);
				markEnd = range.from + startSpaces + captureGroup.length;
				tr.addMark(range.from + startSpaces, markEnd, config.type.create(attributes || {}));
				tr.removeStoredMark(config.type);
			}
		}
	});
}
function canInsertNode(state, nodeType) {
	const { selection } = state;
	const { $from } = selection;
	if (selection instanceof NodeSelection) {
		const index = $from.index();
		return $from.parent.canReplaceWith(index, index + 1, nodeType);
	}
	let depth = $from.depth;
	while (depth >= 0) {
		const index = $from.index(depth);
		if ($from.node(depth).contentMatchAt(index).matchType(nodeType)) return true;
		depth -= 1;
	}
	return false;
}
function escapeForRegEx(string) {
	return string.replace(/[-/\\^$*+?.()|[\]{}]/g, "\\$&");
}
function isString(value) {
	return typeof value === "string";
}
/**
* Build an paste rule that adds a node when the
* matched text is pasted into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#paste-rules
*/
function nodePasteRule(config) {
	return new PasteRule({
		find: config.find,
		handler({ match, chain, range, pasteEvent }) {
			const attributes = callOrReturn(config.getAttributes, void 0, match, pasteEvent);
			const content = callOrReturn(config.getContent, void 0, attributes);
			if (attributes === false || attributes === null) return null;
			const node = {
				type: config.type.name,
				attrs: attributes
			};
			if (content) node.content = content;
			if (match.input) chain().deleteRange(range).insertContentAt(range.from, node);
		}
	});
}
/**
* Build an paste rule that replaces text when the
* matched text is pasted into it.
* @see https://tiptap.dev/docs/editor/extensions/custom-extensions/extend-existing#paste-rules
*/
function textPasteRule(config) {
	return new PasteRule({
		find: config.find,
		handler: ({ state, range, match }) => {
			let insert = config.replace;
			let start = range.from;
			const end = range.to;
			if (match[1]) {
				const offset = match[0].lastIndexOf(match[1]);
				insert += match[0].slice(offset + match[1].length);
				start += offset;
				const cutOff = start - end;
				if (cutOff > 0) {
					insert = match[0].slice(offset - cutOff, offset) + insert;
					start = end;
				}
			}
			state.tr.insertText(insert, start, end);
		}
	});
}
var Tracker = class {
	constructor(transaction) {
		this.transaction = transaction;
		this.currentStep = this.transaction.steps.length;
	}
	map(position) {
		let deleted = false;
		return {
			position: this.transaction.steps.slice(this.currentStep).reduce((newPosition, step) => {
				const mapResult = step.getMap().mapResult(newPosition);
				if (mapResult.deleted) deleted = true;
				return mapResult.pos;
			}, position),
			deleted
		};
	}
};
//#endregion
export { getTextSerializersFromSchema as $, generateText as A, pasteRulesPlugin as At, getMarksBetween as B, wrappingInputRule as Bt, findChildrenInRange as C, markPasteRule as Ct, fromString as D, nodeInputRule as Dt, findParentNodeClosestToPos as E, minMax as Et, getExtensionField as F, selectionToInsertionEnd as Ft, getSchema as G, getNodeAttributes as H, getHTMLFromFragment as I, splitExtensions as It, getSchemaTypeNameByName as J, getSchemaByResolvedExtensions as K, getMarkAttributes as L, textInputRule as Lt, getAttributesFromExtensions as M, removeDuplicates as Mt, getChangedRanges as N, resolveFocusPosition as Nt, generateHTML as O, nodePasteRule as Ot, getDebugJSON as P, rewriteUnknownContent as Pt, getTextContentFromNodes as Q, getMarkRange as R, textPasteRule as Rt, findChildren as S, markInputRule as St, findParentNode as T, mergeDeep as Tt, getNodeType as U, getNodeAtPosition as V, keydownHandler as Vt, getRenderedAttributes as W, getText as X, getSplittedAttributes as Y, getTextBetween as Z, createStyleTag as _, isRegExp as _t, Mark as a, isAtStartOfNode as at, elementFromString as b, isTextSelection as bt, NodeView as c, isFunction as ct, callOrReturn as d, isMarkActive as dt, index as et, canInsertNode as f, isNodeActive as ft, createNodeFromContent as g, isPlainObject as gt, createDocument as h, isNumber as ht, InputRule as i, isAtEndOfNode as it, getAttributes as j, posToDOMRect as jt, generateJSON as k, objectIncludes as kt, PasteRule as l, isList as lt, createChainableState as m, isNodeSelection as mt, Editor as n, inputRulesPlugin as nt, Node as o, isEmptyObject as ot, combineTransactionSteps as p, isNodeEmpty as pt, getSchemaTypeByName as q, Extension as r, isActive as rt, NodePos as s, isExtensionRulesEnabled as st, CommandManager as t, injectExtensionAttributesToParseRule as tt, Tracker as u, isMacOS as ut, defaultBlockAt as v, isSafari as vt, findDuplicates as w, mergeAttributes as wt, escapeForRegEx as x, isiOS as xt, deleteProps as y, isString as yt, getMarkType as z, textblockTypeInputRule as zt };
