/**
 * Progressive segmented TTS playback coordinator for local Kokoro-82M synthesis.
 *
 * Splits text into short natural chunks, prefetches the next segment while the
 * current one plays, and supports immediate cancellation.
 */
import { ref } from "vue";
import { commandErrorMessage, voiceSynthesizeSegment } from "@/contracts/commands";
import { prepareTextForSpeech, splitTextForSpeech } from "./voice-text";

export const speakingKey = ref<string | null>(null);
export const speakingLoading = ref(false);
export const speakingError = ref<string | null>(null);

let activeRunId = 0;
let audioCtx: AudioContext | null = null;
let activeSource: AudioBufferSourceNode | null = null;

function ensureAudioContext(): AudioContext {
  if (!audioCtx || audioCtx.state === "closed") {
    audioCtx = new AudioContext();
  }
  if (audioCtx.state === "suspended") {
    void audioCtx.resume();
  }
  return audioCtx;
}

function base64ToArrayBuffer(b64: string): ArrayBuffer {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes.buffer;
}

export function stopSpeaking(): void {
  activeRunId += 1;
  if (activeSource) {
    try {
      activeSource.stop();
      activeSource.disconnect();
    } catch {
      // already stopped
    }
    activeSource = null;
  }
  speakingKey.value = null;
  speakingLoading.value = false;
}

export async function speakText(
  key: string,
  rawText: string,
  options: {
    modelId?: string | null;
    voiceId?: string | null;
    speed?: number | null;
    language?: string | null;
  } = {},
): Promise<void> {
  if (speakingKey.value === key) {
    stopSpeaking();
    return;
  }

  stopSpeaking();
  const runId = activeRunId;
  speakingError.value = null;

  const prepared = prepareTextForSpeech(rawText, {
    language: options.language ?? null,
  });
  const segments = splitTextForSpeech(prepared);
  if (segments.length === 0) return;

  speakingKey.value = key;
  speakingLoading.value = true;

  // Create/resume AudioContext synchronously inside the user gesture before any await
  // so WKWebView (macOS) never blocks playback under autoplay policies.
  const ctx = ensureAudioContext();

  try {
    if (ctx.state === "suspended") {
      await ctx.resume();
    }
    let pendingNext = voiceSynthesizeSegment({
      text: segments[0],
      modelId: options.modelId ?? null,
      voiceId: options.voiceId ?? null,
      speed: options.speed ?? null,
      language: options.language ?? null,
    });

    for (let i = 0; i < segments.length; i++) {
      const result = await pendingNext;
      if (runId !== activeRunId) return;

      // Start synthesizing segment i+1 while segment i plays.
      if (i + 1 < segments.length) {
        pendingNext = voiceSynthesizeSegment({
          text: segments[i + 1],
          modelId: options.modelId ?? null,
          voiceId: options.voiceId ?? null,
          speed: options.speed ?? null,
          language: options.language ?? null,
        });
      }

      const decoded = await ctx.decodeAudioData(base64ToArrayBuffer(result.wavBase64));
      if (runId !== activeRunId) return;

      speakingLoading.value = false;
      await new Promise<void>((resolve) => {
        const source = ctx.createBufferSource();
        source.buffer = decoded;
        source.connect(ctx.destination);
        activeSource = source;
        source.onended = () => {
          if (activeSource === source) {
            activeSource = null;
          }
          resolve();
        };
        source.start(0);
      });

      if (runId !== activeRunId) return;
    }
  } catch (cause) {
    if (runId === activeRunId) {
      speakingError.value = commandErrorMessage(cause);
    }
  } finally {
    if (runId === activeRunId) {
      speakingKey.value = null;
      speakingLoading.value = false;
    }
  }
}
