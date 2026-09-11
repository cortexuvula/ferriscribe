// Fail-closed boundary: no real Tauri bridge, OS, settings, DB or provider.
export const audit = {calls: [] as {command:string,args:unknown}[],unknown:[] as string[],copies:[] as string[],speed:[] as unknown[],navigation:0,expectedFailures:[] as string[]};
let saved: any;
let mode = 'success';
export function seed(recording:any) { saved=structuredClone(recording); }
export function setMode(value:string) { mode=value; }
export async function invoke(command:string,args:any={}) {
 audit.calls.push({command,args});
 switch(command) {
 case 'list_letter_audiences': return [{id:'builtin-patient',name:'Synthetic patient audience',instructions:'Synthetic',is_builtin:true}];
 case 'list_context_templates': return [];
 case 'list_condition_chips': return [];
 case 'subscribe_condition_chips': return null; // Simulated SSE subscription; never starts network.
 case 'get_recording': if(args.id!==saved.id) throw new Error('Unexpected fixture recording'); return structuredClone(saved);
 case 'list_recordings': return [];
 case 'generate_soap':
 case 'generate_referral':
 case 'generate_letter':
 case 'generate_peer_discussion': {
   if(args.recordingId!==saved.id) throw new Error('Unexpected fixture recording');
   if(mode==='pending') return await new Promise(()=>{});
   if(mode==='failure') { audit.expectedFailures.push('SYNTHETIC generation failed'); throw new Error('SYNTHETIC generation failed'); }
   const field=({generate_soap:'soap_note',generate_referral:'referral',generate_letter:'letter',generate_peer_discussion:'peer_discussion'} as const)[command];
   saved[field]='SYNTHETIC generated '+field+' output. Review this fixture text before use.';
   return saved[field];
 }
 default: audit.unknown.push(command); throw new Error('UNMOCKED TAURI CALL: '+command);
 }
}
function blocked(name:string):never { audit.unknown.push(name);throw new Error('UNMOCKED TAURI FUNCTION: '+name); }
export const getCurrentWebview=()=>({onDragDropEvent:async()=>{audit.calls.push({command:'onDragDropEvent',args:null});return ()=>{};}});
export async function listen(name:string) { if(!['ocr-progress','condition-chips-changed'].includes(name)) return blocked('listen:'+name); audit.calls.push({command:'listen',args:name});return ()=>{}; }
export async function writeText(text:string) {audit.copies.push(text);}
export const open=()=>blocked('open');
export const convertFileSrc=()=>blocked('convertFileSrc');
export const emit=()=>blocked('emit');
