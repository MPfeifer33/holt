/**
 * HIL Alert Sound — Web Audio API tone generator.
 * Plays a short, attention-grabbing two-tone chime when an agent needs human input.
 * No external audio files required.
 */

import { ALERT_COOLDOWN_MS, ALERT_VOLUME } from '$lib/constants';

let audioContext: AudioContext | null = null;
let lastPlayTime = 0;
const COOLDOWN_MS = ALERT_COOLDOWN_MS;

function getContext(): AudioContext {
    if (!audioContext) {
        audioContext = new AudioContext();
    }
    return audioContext;
}

/**
 * Play the HIL alert chime — a gentle two-tone ascending notification.
 * Returns immediately if audio is on cooldown or context can't be created.
 */
export function playHilAlert(): void {
    const now = Date.now();
    if (now - lastPlayTime < COOLDOWN_MS) return;
    lastPlayTime = now;

    try {
        const ctx = getContext();

        // Resume context if suspended (browser autoplay policy)
        if (ctx.state === 'suspended') {
            ctx.resume();
        }

        const masterGain = ctx.createGain();
        masterGain.gain.value = ALERT_VOLUME;
        masterGain.connect(ctx.destination);

        // Tone 1: lower note (C5 = 523 Hz)
        playTone(ctx, masterGain, 523, ctx.currentTime, 0.12);
        // Tone 2: higher note (E5 = 659 Hz), slightly delayed
        playTone(ctx, masterGain, 659, ctx.currentTime + 0.13, 0.12);
        // Tone 3: resolution note (G5 = 784 Hz)
        playTone(ctx, masterGain, 784, ctx.currentTime + 0.26, 0.18);
    } catch {
        // Audio not available — fail silently
    }
}

function playTone(
    ctx: AudioContext,
    destination: AudioNode,
    frequency: number,
    startTime: number,
    duration: number,
): void {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();

    osc.type = 'sine';
    osc.frequency.value = frequency;

    // Smooth attack and release envelope
    gain.gain.setValueAtTime(0, startTime);
    gain.gain.linearRampToValueAtTime(1, startTime + 0.02); // 20ms attack
    gain.gain.setValueAtTime(1, startTime + duration - 0.04);
    gain.gain.linearRampToValueAtTime(0, startTime + duration); // 40ms release

    osc.connect(gain);
    gain.connect(destination);

    osc.start(startTime);
    osc.stop(startTime + duration);
}
