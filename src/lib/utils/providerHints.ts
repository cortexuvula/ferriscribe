/**
 * Plain-language guidance for bringing each local AI provider's server up,
 * shown next to the model list in Settings → Models when the list can't be
 * fetched. A dead server otherwise reads as an empty dropdown or a lone
 * placeholder model, with no clue that Ollama / LM Studio / oMLX simply
 * isn't running.
 */

const START_HINTS: Record<string, string> = {
  ollama:
    'Start the Ollama app (menu-bar icon) or run `ollama serve`, and make sure at least one model is pulled (`ollama list`).',
  lmstudio: 'Start LM Studio and turn on its local server (Developer → Start Server).',
  omlx: 'Start the oMLX app.'
};

const LABELS: Record<string, string> = {
  ollama: 'Ollama',
  lmstudio: 'LM Studio',
  omlx: 'oMLX'
};

export function providerLabel(provider: string): string {
  return LABELS[provider] ?? provider;
}

/** How to start the provider's server on this machine. */
export function providerStartHint(provider: string): string {
  return (
    START_HINTS[provider] ??
    `Check that the ${providerLabel(provider)} server is running at the address shown above, then click Refresh.`
  );
}

/**
 * Paired office clients fetch models through the office server's proxies,
 * not from a local server — when the office machine doesn't run the
 * provider, the local start hint would send the user down the wrong path.
 */
export function officeServedHint(provider: string): string {
  return `This provider is served by the office server you are paired with — that machine is not running ${providerLabel(
    provider
  )} right now. Start it on the office machine (or pick a provider the office serves), then click Refresh.`;
}
