import { invoke } from '@tauri-apps/api/core';
import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';
import type { AgentResponse } from '../types';

export interface ChatMessageInput {
  role: string;
  content: string;
}

export async function chatSend(
  messages: ChatMessageInput[],
  model?: string,
  systemPrompt?: string
): Promise<string> {
  return invokeWithOfflineHandling('chat_send', {
    messages,
    model: model ?? null,
    systemPrompt: systemPrompt ?? null,
  });
}

/** A document attached to the chat conversation (OCR'd on the frontend). */
export interface ChatDocument {
  name: string;
  content: string;
}

export interface ChatStreamOptions {
  model?: string;
  systemPrompt?: string;
  /** Attached documents; the backend appends them to the system prompt. */
  documents?: ChatDocument[];
}

export async function chatStream(
  messages: ChatMessageInput[],
  opts: ChatStreamOptions = {}
): Promise<void> {
  return invokeWithOfflineHandling('chat_stream', {
    messages,
    model: opts.model ?? null,
    systemPrompt: opts.systemPrompt ?? null,
    documents: opts.documents && opts.documents.length > 0 ? opts.documents : null,
  });
}

/** Drop the backend's conversation document index (chart-review mode). */
export async function chatClearDocuments(): Promise<void> {
  return invoke('chat_clear_documents');
}

export async function chatWithAgent(
  message: string,
  agentName: string,
  conversationHistory?: ChatMessageInput[]
): Promise<AgentResponse> {
  return invokeWithOfflineHandling('chat_with_agent', {
    message,
    agentName,
    conversationHistory: conversationHistory ?? null,
  });
}

export async function listAiProviders(): Promise<string[]> {
  return invoke('list_ai_providers');
}

export async function setActiveProvider(name: string): Promise<boolean> {
  return invoke('set_active_provider', { name });
}

export async function reinitProviders(): Promise<string[]> {
  return invoke('reinit_providers');
}

export interface ModelInfo {
  id: string;
  name: string;
  provider: string;
  max_tokens: number;
  supports_tools: boolean;
  supports_streaming: boolean;
}

export async function listModels(providerName?: string): Promise<ModelInfo[]> {
  return invoke('list_models', { providerName: providerName ?? null });
}
