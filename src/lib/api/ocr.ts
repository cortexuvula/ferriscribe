import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';

export interface OcrPageResult {
  filename: string;
  text: string;
  page_count: number;
}

/** OCR document files and return extracted text. */
export async function ocrDocuments(filePaths: string[]): Promise<OcrPageResult[]> {
  return invokeWithOfflineHandling<OcrPageResult[]>('ocr_documents', { filePaths });
}
