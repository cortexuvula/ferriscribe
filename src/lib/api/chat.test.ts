import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  chatSend,
  chatStream,
  chatWithAgent,
  listAiProviders,
  setActiveProvider,
  reinitProviders,
  listModels,
} from './chat';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

const msgs = [{ role: 'user', content: 'hello' }];

describe('chat api', () => {
  it('chatSend null-coalesces optional fields when omitted', async () => {
    await chatSend(msgs);
    expect(invokeMock).toHaveBeenCalledWith('chat_send', {
      messages: msgs,
      model: null,
      systemPrompt: null,
    });
  });

  it('chatSend forwards model and systemPrompt when provided', async () => {
    await chatSend(msgs, 'gpt-X', 'be brief');
    expect(invokeMock).toHaveBeenCalledWith('chat_send', {
      messages: msgs,
      model: 'gpt-X',
      systemPrompt: 'be brief',
    });
  });

  it('chatStream invokes chat_stream with the same shape and null-coalesces optionals', async () => {
    await chatStream(msgs, 'm', 's');
    expect(invokeMock).toHaveBeenCalledWith('chat_stream', {
      messages: msgs,
      model: 'm',
      systemPrompt: 's',
    });
    invokeMock.mockReset();
    await chatStream(msgs);
    expect(invokeMock).toHaveBeenCalledWith('chat_stream', {
      messages: msgs,
      model: null,
      systemPrompt: null,
    });
  });

  it('chatWithAgent passes message + agentName + history, null-coalesces history when omitted', async () => {
    await chatWithAgent('hi', 'soap-agent', msgs);
    expect(invokeMock).toHaveBeenCalledWith('chat_with_agent', {
      message: 'hi',
      agentName: 'soap-agent',
      conversationHistory: msgs,
    });
    invokeMock.mockReset();
    await chatWithAgent('hi', 'soap-agent');
    expect(invokeMock).toHaveBeenCalledWith('chat_with_agent', {
      message: 'hi',
      agentName: 'soap-agent',
      conversationHistory: null,
    });
  });

  it('listAiProviders / reinitProviders invoke without args', async () => {
    await listAiProviders();
    await reinitProviders();
    expect(invokeMock).toHaveBeenNthCalledWith(1, 'list_ai_providers');
    expect(invokeMock).toHaveBeenNthCalledWith(2, 'reinit_providers');
  });

  it('setActiveProvider passes name', async () => {
    await setActiveProvider('ollama');
    expect(invokeMock).toHaveBeenCalledWith('set_active_provider', { name: 'ollama' });
  });

  it('listModels null-coalesces providerName when omitted', async () => {
    await listModels();
    expect(invokeMock).toHaveBeenCalledWith('list_models', { providerName: null });
    invokeMock.mockReset();
    await listModels('ollama');
    expect(invokeMock).toHaveBeenCalledWith('list_models', { providerName: 'ollama' });
  });
});
