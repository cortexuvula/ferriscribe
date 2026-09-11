import '../../src/app.css';
import { mount } from 'svelte';
import { settings } from '../../src/lib/stores/settings.svelte';
import { settingsNav } from '../../src/lib/stores/settingsNav.svelte';
import { seed, audit } from './browser-tauri';
const params = new URLSearchParams(location.search);
const theme = params.get('theme') === 'light' ? 'light' : 'dark';
seed({...JSON.parse(JSON.stringify(settings.state)), theme, ai_provider:'ollama',ai_model:'qwen3:8b',onboarding_completed:true});
await settings.load();
document.documentElement.dataset.theme = theme;
if (params.get('case') === 'restart-public') {
  settings.state.allow_public_endpoint = true;
  const { updater } = await import('../../src/lib/stores/updater.svelte');
  updater.pendingRestart = '99.0.0-fixture';
}
settingsNav.state.lastSection = (params.get('page') || 'general') as any;
Object.assign(window, { fixtureAudit:audit, fixtureSettings:settings, fixtureNav:settingsNav });
const { default: Harness } = await import('./browser-Harness.svelte');
mount(Harness, {target: document.getElementById('fixture')!});
