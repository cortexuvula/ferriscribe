import { invoke } from '@tauri-apps/api/core';

export interface OcrPageResult {
  filename: string;
  text: string;
  page_count: number;
}

/** OCR document files and return extracted text. */
export async function ocrDocuments(filePaths: string[]): Promise<OcrPageResult[]> {
  return invoke<OcrPageResult[]>('ocr_documents', { filePaths });
}
