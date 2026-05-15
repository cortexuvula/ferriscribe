import type { Recording, RecordingSummary } from '../types';
import {
  listRecordings,
  getRecording,
  searchRecordings,
  deleteRecording,
  deleteAllRecordings,
} from '../api/recordings';

class RecordingsStore {
  list = $state<RecordingSummary[]>([]);
  loading = $state<boolean>(false);
  searchQuery = $state<string>('');
  selectedRecording = $state<Recording | null>(null);

  async load(limit = 50, offset = 0): Promise<void> {
    this.loading = true;
    try {
      const items = await listRecordings(limit, offset);
      this.list = items;
    } catch (err) {
      console.error('Failed to load recordings:', err);
    } finally {
      this.loading = false;
    }
  }

  async search(query: string): Promise<void> {
    this.searchQuery = query;
    this.loading = true;
    try {
      if (query.trim() === '') {
        const items = await listRecordings();
        this.list = items;
      } else {
        const results = await searchRecordings(query);
        // Map full Recording to RecordingSummary shape
        const summaries: RecordingSummary[] = results.map((r) => ({
          id: r.id,
          filename: r.filename,
          patient_name: r.patient_name,
          status: r.status,
          duration_seconds: r.duration_seconds,
          created_at: r.created_at,
          tags: r.tags,
          has_transcript: r.transcript !== null,
          has_soap_note: r.soap_note !== null,
          has_referral: r.referral !== null,
          has_letter: r.letter !== null,
        }));
        this.list = summaries;
      }
    } catch (err) {
      console.error('Failed to search recordings:', err);
    } finally {
      this.loading = false;
    }
  }

  async remove(id: string): Promise<void> {
    try {
      await deleteRecording(id);
      this.list = this.list.filter((r) => r.id !== id);
      // Clear selected if it was the deleted one
      if (this.selectedRecording?.id === id) {
        this.selectedRecording = null;
      }
    } catch (err) {
      console.error('Failed to delete recording:', err);
      throw err;
    }
  }

  async removeAll(): Promise<number> {
    try {
      const count = await deleteAllRecordings();
      this.list = [];
      this.selectedRecording = null;
      return count;
    } catch (err) {
      console.error('Failed to delete all recordings:', err);
      throw err;
    }
  }
}

export const recordings = new RecordingsStore();

export async function selectRecording(id: string): Promise<void> {
  try {
    const recording = await getRecording(id);
    recordings.selectedRecording = recording;
  } catch (err) {
    console.error('Failed to select recording:', err);
    throw err;
  }
}
