// Every Tauri import resolves here BEFORE production modules load. No bridge exists.
export const audit = { calls: [] as {command:string,args:unknown}[], unknown: [] as string[] };
let fixture: any;
export function seed(config: unknown) { fixture = structuredClone(config); }
export async function invoke(command: string, args: any = {}) {
  audit.calls.push({ command, args });
  switch(command) {
    case 'get_settings': if (!fixture) throw new Error('Fixture not seeded'); return structuredClone(fixture);
    case 'save_settings': if (new URLSearchParams(location.search).get('case') === 'save-failure') throw new Error('SYNTHETIC FIXTURE save failed'); fixture = structuredClone(args.config); return null;
    case 'get_api_key': return null; // SYNTHETIC absence, never keychain
    case 'paired_endpoint': return null;
    case 'list_models': if (new URLSearchParams(location.search).get('case') === 'offline') throw new Error('SYNTHETIC FIXTURE provider offline'); return ['qwen3:8b','qwen3:1.7b'].map(id=>({id,name: `${id} (fixture)`,provider:args.providerName}));
    case 'set_active_provider': return null;
    case 'list_specialty_packs': return [];
    case 'get_default_prompt': return `SYNTHETIC FIXTURE — ${args.docType}\n\nThis is placeholder text for layout and interaction verification.\nNo patient information or clinical instructions are loaded.\n\n{{transcript}}`;
    default: audit.unknown.push(command); throw new Error(`UNMOCKED TAURI CALL: ${command}`);
  }
}
function blocked(name:string): never { audit.unknown.push(name); throw new Error(`UNMOCKED TAURI FUNCTION: ${name}`); }
export async function listen(name:string) { audit.calls.push({command:'listen',args:name}); return () => {}; }
export const emit = () => blocked('emit');
export const open = () => blocked('open');
export const save = () => blocked('save');
export const revealItemInDir = () => blocked('revealItemInDir');
export const openUrl = () => blocked('openUrl');
export const writeText = () => blocked('writeText');
export const check = () => blocked('check');
export const relaunch = () => blocked('relaunch');
export const convertFileSrc = () => blocked('convertFileSrc');
