// @vitest-environment jsdom
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/svelte';
import {afterEach,it,expect,vi} from 'vitest';
import GenerateTab from './GenerateTab.svelte';
import {recordings} from '../stores/recordings.svelte';
import {generation} from '../stores/generation.svelte';
import {invoke} from '@tauri-apps/api/core';
vi.mock('@tauri-apps/api/core',()=>({invoke:vi.fn()}));
vi.mock('@tauri-apps/api/webview',()=>({getCurrentWebview:()=>({onDragDropEvent:async()=>()=>{}})}));
vi.mock('@tauri-apps/api/event',()=>({listen:async()=>()=>{}}));
vi.mock('@tauri-apps/plugin-dialog',()=>({open:async()=>null}));
vi.mock('@tauri-apps/plugin-clipboard-manager',()=>({writeText:async()=>{}}));
vi.mock('../utils/notificationSound',()=>({playSoapCompleteChime:()=>{}}));
afterEach(cleanup);
it('drops a fresh response for the SAME recording after a notes edit',async()=>{
 const pending: Array<(value:unknown)=>void>=[];
 vi.mocked(invoke).mockImplementation(async command=>command==='get_generation_freshness'?new Promise(resolve=>pending.push(resolve)):[]);
 recordings.selectedRecording={id:'synthetic-a',filename:'synthetic.wav',transcript:'Synthetic transcript',soap_note:'Synthetic SOAP',referral:null,letter:null,peer_discussion:null,metadata:{context:'Synthetic alpha'},status:{status:'pending'},tags:[],created_at:''} as never;
 generation.finish(); generation.clearError();
 render(GenerateTab);
 await waitFor(()=>expect(pending).toHaveLength(1));
 await fireEvent.input(screen.getByLabelText('Notes'),{target:{value:'Synthetic beta'}});
 await waitFor(()=>expect(pending).toHaveLength(2));
 const fresh={status:'fresh',reasons:[]};
 pending[0]({soap:fresh,referral:fresh,letter:fresh,peer_discussion:fresh});
 await new Promise(resolve=>setTimeout(resolve,30));
 expect(screen.queryByText('Current')).toBeNull();
 expect(screen.getByText('Checking freshness…')).toBeTruthy();
});
