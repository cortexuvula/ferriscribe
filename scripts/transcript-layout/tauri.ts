// All native imports resolve here. Unexpected commands fail the gate.
export const audit = {
  calls: [] as Record<string, unknown>[],
  copied: [] as string[],
  unknown: [] as string[],
};
Object.assign(window, { audit });
export async function invoke(command: string, args: Record<string, unknown> = {}) {
  audit.calls.push({ command, args });
  if (['save_recording_field', 'register_pending_edit', 'clear_pending_edit'].includes(command)) return null;
  audit.unknown.push(command);
  throw new Error('BLOCKED synthetic harness IPC: ' + command);
}
export async function listen(event: string) {
  audit.calls.push({ command: 'listen', event });
  return () => {};
}
export async function writeText(text: string) { audit.copied.push(text); }
export async function readText() { throw new Error('NO CLIPBOARD READ'); }
export function convertFileSrc() { throw new Error('NO AUDIO ACCESS'); }
export async function open() {
  audit.unknown.push('native-action');
  throw new Error('NO NATIVE ACTION');
}
export const save = open;
export const openUrl = open;
export const revealItemInDir = open;
export const check = open;
export const relaunch = open;
export function getCurrentWebview() { return { onDragDropEvent: async () => () => {} }; }
export function getCurrentWindow() { return { listen, onFocusChanged: async () => () => {} }; }
