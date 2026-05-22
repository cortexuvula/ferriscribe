import type { LetterAudience } from '../types/letterAudience';
import {
  listLetterAudiences,
  upsertLetterAudience as apiUpsert,
  deleteLetterAudience as apiDelete,
} from '../api/letterAudiences';

function createLetterAudiencesStore() {
  let audiences = $state<LetterAudience[]>([]);
  let loading = $state(false);
  let error = $state<string | null>(null);

  async function list() {
    loading = true;
    error = null;
    try {
      audiences = await listLetterAudiences();
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load audiences';
      console.error('Failed to load letter audiences:', e);
    } finally {
      loading = false;
    }
  }

  async function upsert(audience: LetterAudience) {
    try {
      const updated = await apiUpsert(audience);
      const index = audiences.findIndex((a) => a.id === updated.id);
      if (index >= 0) {
        audiences[index] = updated;
      } else {
        audiences.push(updated);
      }
      return updated;
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to save audience';
      error = msg;
      throw e;
    }
  }

  async function remove(id: string) {
    try {
      await apiDelete(id);
      audiences = audiences.filter((a) => a.id !== id);
    } catch (e) {
      const msg = e instanceof Error ? e.message : 'Failed to delete audience';
      error = msg;
      throw e;
    }
  }

  return {
    get audiences() {
      return audiences;
    },
    get loading() {
      return loading;
    },
    get error() {
      return error;
    },
    list,
    upsert,
    delete: remove,
  };
}

export const letterAudiences = createLetterAudiencesStore();
