#!/usr/bin/env python3
"""Generate the synthetic camera-shutter click sound.

Synthesizes a ~0.3s mono 22050 Hz 16-bit shutter-click WAV entirely in code
(stdlib `wave`/`math`/`random` only, no Pillow, no new dependency) so there
is zero licensing concern for public release: two short white-noise bursts
with exponential amplitude decay -- a sharp ~15ms attack burst followed by a
softer ~10ms burst ~40-60ms later, both decaying to near-zero.

Re-run this script any time the shutter sound needs to be regenerated.

Requires: Python stdlib only.
"""

import math
import random
import wave
from pathlib import Path

SAMPLE_RATE = 22050
OUT_PATH = Path(__file__).resolve().parent.parent / "resources" / "shutter.wav"

# Total duration of the generated clip, in seconds.
TOTAL_DURATION_S = 0.35

# Burst 1: sharp attack, fast decay. Durations/decays sized so the clicks
# carry enough acoustic energy to be clearly audible at normal system volume
# (the original 15ms/10ms bursts were nearly inaudible in practice).
BURST1_START_S = 0.0
BURST1_DURATION_S = 0.055
BURST1_DECAY = 14.0  # higher = faster decay
BURST1_AMPLITUDE = 1.0

# Burst 2: softer, starts ~80ms after burst 1.
BURST2_START_S = 0.08
BURST2_DURATION_S = 0.040
BURST2_DECAY = 18.0
BURST2_AMPLITUDE = 0.7

# Timbre shaping: raw white noise sounds like a knock on wood. A camera
# shutter is bright and metallic, so the noise is high-pass filtered
# (removing the low "thump" content) and each burst gets a short damped
# sine "ping" that adds the metallic mechanism character.
HIGHPASS_ALPHA = 0.85  # one-pole high-pass coefficient (higher = brighter)
PING1_FREQ_HZ = 4200.0
PING1_AMPLITUDE = 0.45
PING2_FREQ_HZ = 2800.0
PING2_AMPLITUDE = 0.35


def _burst_envelope(t: float, start: float, duration: float, decay: float) -> float:
    """Returns the envelope amplitude at time `t` for a burst starting at
    `start`, spanning roughly `duration` seconds with exponential decay."""
    if t < start:
        return 0.0
    dt = t - start
    if dt > duration * 6:  # decayed to near-zero well past nominal duration
        return 0.0
    return math.exp(-decay * dt / duration)


def synthesize() -> bytes:
    random.seed(42)  # deterministic output across regenerations
    n_samples = int(SAMPLE_RATE * TOTAL_DURATION_S)
    values = []
    prev_noise = 0.0
    hp_noise_prev = 0.0
    for i in range(n_samples):
        t = i / SAMPLE_RATE
        env1 = _burst_envelope(t, BURST1_START_S, BURST1_DURATION_S, BURST1_DECAY)
        env2 = _burst_envelope(t, BURST2_START_S, BURST2_DURATION_S, BURST2_DECAY)

        # One-pole high-pass filtered noise: bright "tsk" instead of a
        # low knock. y[i] = a*(y[i-1] + x[i] - x[i-1])
        noise = random.uniform(-1.0, 1.0)
        hp_noise = HIGHPASS_ALPHA * (hp_noise_prev + noise - prev_noise)
        prev_noise = noise
        hp_noise_prev = hp_noise

        value = hp_noise * (BURST1_AMPLITUDE * env1 + BURST2_AMPLITUDE * env2)

        # Damped sine pings, one per burst, for the metallic mechanism tone.
        if env1 > 0.0:
            value += (
                PING1_AMPLITUDE
                * env1
                * math.sin(2.0 * math.pi * PING1_FREQ_HZ * (t - BURST1_START_S))
            )
        if env2 > 0.0:
            value += (
                PING2_AMPLITUDE
                * env2
                * math.sin(2.0 * math.pi * PING2_FREQ_HZ * (t - BURST2_START_S))
            )
        values.append(value)

    # Normalize to ~90% full scale so loudness is stable across tweaks.
    peak = max(abs(v) for v in values) or 1.0
    scale = 0.9 / peak
    samples = bytearray()
    for value in values:
        sample = int(max(-1.0, min(1.0, value * scale)) * 32767)
        samples += sample.to_bytes(2, byteorder="little", signed=True)
    return bytes(samples)


def main() -> None:
    pcm = synthesize()

    OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(OUT_PATH), "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)  # 16-bit
        wf.setframerate(SAMPLE_RATE)
        wf.writeframes(pcm)

    size_kb = OUT_PATH.stat().st_size / 1024
    print(f"Wrote {OUT_PATH} ({size_kb:.1f} KB, {SAMPLE_RATE} Hz, mono, 16-bit)")


if __name__ == "__main__":
    main()
