import {
  listContextTemplates,
  type ContextTemplate,
} from '../api/contextTemplates';

class ContextTemplatesStore {
  list = $state<ContextTemplate[]>([]);

  async load(): Promise<void> {
    try {
      const items = await listContextTemplates();
      this.list = items;
    } catch (err) {
      console.error('Failed to load context templates:', err);
    }
  }
}

export const contextTemplates = new ContextTemplatesStore();
