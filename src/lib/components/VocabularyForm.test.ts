// @vitest-environment jsdom
/**
 * VocabularyForm — regression tests for the save-error rendering.
 *
 * The form previously stringified the invoke rejection with `String(err)`,
 * which renders Tauri's structured errors (the `{kind, message}` AppError
 * shape) as "[object Object]". These tests pin the fix: a rejection object
 * with a `message` field must render as that message (via formatError).
 *
 * Markup facts these tests rely on (kept in sync with the component):
 *   - Save is `<button class="btn-save">Save</button>`.
 *   - Validation / backend errors render in `<div class="form-error">`.
 *   - Find text input is the first `<input>` in the form grid.
 */
import { it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import VocabularyForm from './VocabularyForm.svelte';

const onSave = vi.fn();

function renderForm() {
  return render(VocabularyForm, {
    props: {
      editing: null,
      categories: [{ value: 'general', label: 'General' }],
      onSave,
      onCancel: () => {},
    },
  });
}

async function fillAndSave(findText: string, replacement: string) {
  const inputs = screen.getAllByRole('textbox');
  await fireEvent.input(inputs[0]!, { target: { value: findText } });
  await fireEvent.input(inputs[1]!, { target: { value: replacement } });
  await fireEvent.click(screen.getByRole('button', { name: 'Save' }));
}

beforeEach(() => {
  onSave.mockReset();
});

afterEach(() => {
  cleanup();
});

it('renders the message field of a structured (object) rejection, not [object Object]', async () => {
  // The shape Tauri delivers when a Rust command returns Err(AppError).
  onSave.mockRejectedValue({
    kind: 'config',
    message: 'A vocabulary entry for "htn" already exists. Edit the existing entry instead.',
  });

  renderForm();
  await fillAndSave('htn', 'hypertension');

  await waitFor(() => expect(screen.getByText(/already exists/i)).toBeTruthy());
  expect(screen.queryByText('[object Object]')).toBeNull();
  expect(onSave).toHaveBeenCalledTimes(1);
});

it('renders plain string rejections as-is', async () => {
  onSave.mockRejectedValue('server unreachable');

  renderForm();
  await fillAndSave('htn', 'hypertension');

  await waitFor(() => expect(screen.getByText('server unreachable')).toBeTruthy());
});

it('requires both fields before invoking save', async () => {
  renderForm();
  const inputs = screen.getAllByRole('textbox');
  await fireEvent.input(inputs[0]!, { target: { value: 'only-find' } });
  await fireEvent.click(screen.getByRole('button', { name: 'Save' }));

  expect(
    screen.getByText('Find and replacement text are required.'),
  ).toBeTruthy();
  expect(onSave).not.toHaveBeenCalled();
});
