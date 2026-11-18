"""Generates the bundled default notification sounds (original synthesized chimes: main task and sub-agent).

Deterministic and fully synthesized here, so it carries no third-party audio rights.
Run: python scripts/make-default-sound.py
"""
import math
import struct
import wave
from pathlib import Path

RATE = 24000
OUT = Path(__file__).resolve().parents[1] / 'src-tauri' / 'resources' / 'default.wav'


def note(freq, start, length, gain):
    for n in range(int(length * RATE)):
        t = n / RATE
        env = min(1.0, t / 0.006) * math.exp(-t * 7.5)  # soft attack, bell-like decay
        tone = math.sin(2 * math.pi * freq * t) + 0.35 * math.sin(2 * math.pi * freq * 2.01 * t) \
            + 0.12 * math.sin(2 * math.pi * freq * 3.02 * t)
        yield int(start * RATE) + n, gain * env * tone


def render(notes, length):
    samples = [0.0] * int(length * RATE)
    for freq, start, gain, dur in notes:
        for index, value in note(freq, start, dur, gain):
            if index < len(samples):
                samples[index] += value
    peak = max(abs(s) for s in samples)
    scale = 0.8 / peak
    return b''.join(struct.pack('<h', int(s * scale * 32767)) for s in samples)


def write(path, frames, seconds):
    with wave.open(str(path), 'wb') as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(frames)
    print('wrote', path, f'{seconds:.2f}s')


# main task: two-note chime (A5 then E6)
write(OUT, render(((880.0, 0.0, 0.5, 0.55), (1318.5, 0.14, 0.45, 0.55)), 0.7), 0.7)
# sub-agent: a single short, softer note so the two are easy to tell apart
write(OUT.with_name('subagent.wav'), render(((1046.5, 0.0, 0.5, 0.3),), 0.38), 0.38)
