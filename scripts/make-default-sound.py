"""Generates the bundled default notification sound (an original two-note chime).

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


samples = [0.0] * int(0.7 * RATE)
for freq, start, gain in ((880.0, 0.0, 0.5), (1318.5, 0.14, 0.45)):  # A5 then E6
    for index, value in note(freq, start, 0.55, gain):
        if index < len(samples):
            samples[index] += value
peak = max(abs(s) for s in samples)
scale = 0.8 / peak
with wave.open(str(OUT), 'wb') as wav:
    wav.setnchannels(1)
    wav.setsampwidth(2)
    wav.setframerate(RATE)
    wav.writeframes(b''.join(struct.pack('<h', int(s * scale * 32767)) for s in samples))
print('wrote', OUT, f'{len(samples) / RATE:.2f}s')
