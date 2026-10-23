#!/usr/bin/env python3
"""Generates Hearth's bundled default sounds into assets/sounds/ (CC0, see LICENSE.md there).

    scripts/make-default-sounds.py

Everything is synthesized here, nothing is sampled. Needs numpy, and ffmpeg with libvorbis for
the ambient loop. The outputs are committed, so this only runs to change them.

- UI sounds: short mono 48 kHz 16-bit WAVs (decoded once at startup, so WAV costs nothing).
- ambient_loop.ogg: a 32 s pad that loops seamlessly by construction. Every partial, every
  slow amplitude swell and the filtered "air" noise complete a whole number of cycles in the
  loop, so the last sample flows into the first and no loop points are needed.
"""

import os
import subprocess
import sys
import wave

import numpy as np

RATE = 48_000
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "assets", "sounds")
LOOP_SECONDS = 32


def write_wav(name, samples):
    data = np.clip(samples, -1.0, 1.0)
    pcm = (data * 32767).astype("<i2").tobytes()
    with wave.open(os.path.join(OUT, name), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(pcm)


def env(n, attack=0.002, decay=0.05):
    """Short attack (no click), exponential decay."""
    t = np.arange(n) / RATE
    a = np.minimum(1.0, t / attack)
    return a * np.exp(-t / decay)


def tone(freq, seconds, decay, partials=((1, 1.0), (2, 0.25), (3, 0.08)), glide=0.0):
    n = int(seconds * RATE)
    t = np.arange(n) / RATE
    # Optional pitch glide (relative change over the sound).
    f = freq * (1.0 + glide * t / seconds)
    phase = 2 * np.pi * np.cumsum(f) / RATE
    out = sum(a * np.sin(k * phase) for k, a in partials)
    fade = np.ones(n)
    tail = int(0.004 * RATE)
    fade[-tail:] = np.linspace(1, 0, tail)
    return out * env(n, decay=decay) * fade


def seq(*parts):
    """Concatenates (offset_seconds, samples) into one buffer."""
    end = max(int(off * RATE) + len(s) for off, s in parts)
    out = np.zeros(end)
    for off, s in parts:
        i = int(off * RATE)
        out[i : i + len(s)] += s
    return out


def level(samples, peak_db):
    return samples / np.max(np.abs(samples)) * 10 ** (peak_db / 20)


def ui_sounds():
    # A soft, short tick: the sound heard most, so it is the quietest and shortest.
    write_wav("move.wav", level(tone(1568, 0.06, 0.012, ((1, 1.0), (2, 0.1))), -16))
    # A dull knock with a slight downward bend: "nothing more this way".
    write_wav("edge_bump.wav", level(tone(196, 0.12, 0.03, ((1, 1.0), (2, 0.4)), glide=-0.25), -12))
    # Up a fifth: yes.
    write_wav(
        "accept.wav",
        level(seq((0, tone(1046.5, 0.12, 0.04)), (0.055, tone(1568, 0.2, 0.07))), -12),
    )
    # Down a fourth: back.
    write_wav(
        "back.wav",
        level(seq((0, tone(1174.7, 0.1, 0.035)), (0.05, tone(880, 0.16, 0.05))), -13),
    )
    # Three rising notes, airy.
    write_wav(
        "overlay_open.wav",
        level(
            seq(
                (0.0, tone(659.3, 0.3, 0.12)),
                (0.05, tone(987.8, 0.3, 0.12)),
                (0.1, tone(1318.5, 0.35, 0.15)),
            ),
            -12,
        ),
    )
    write_wav(
        "overlay_close.wav",
        level(
            seq(
                (0.0, tone(1318.5, 0.25, 0.08)),
                (0.045, tone(987.8, 0.25, 0.08)),
                (0.09, tone(659.3, 0.3, 0.1)),
            ),
            -13,
        ),
    )
    # Reserved names: a bright arpeggio, and a longer one.
    arp = [523.3, 659.3, 784.0, 1046.5]
    write_wav(
        "achievement.wav",
        level(seq(*[(i * 0.08, tone(f, 0.6, 0.2)) for i, f in enumerate(arp)]), -10),
    )
    arp2 = arp + [1318.5, 1568.0]
    write_wav(
        "mastery.wav",
        level(seq(*[(i * 0.09, tone(f, 1.0, 0.35)) for i, f in enumerate(arp2)]), -9),
    )


def ambient():
    n = LOOP_SECONDS * RATE
    t = np.arange(n) / RATE
    rng = np.random.default_rng(7)

    def q(freq):
        """The nearest frequency with a whole number of cycles in the loop."""
        return round(freq * LOOP_SECONDS) / LOOP_SECONDS

    def swell(cycles, phase):
        """A slow 0.35–1.0 swell completing `cycles` cycles per loop."""
        return 0.675 + 0.325 * np.sin(2 * np.pi * cycles * t / LOOP_SECONDS + phase)

    # D major 9 spread wide and low: D2 A2 D3 F#3 A3 E4 C#5.
    notes = [73.42, 110.0, 146.83, 185.0, 220.0, 329.63, 554.37]
    weights = [0.9, 0.7, 0.8, 0.55, 0.6, 0.35, 0.18]
    channels = []
    for ch in range(2):
        out = np.zeros(n)
        for i, (f, w) in enumerate(zip(notes, weights)):
            phase = rng.uniform(0, 2 * np.pi)
            # A detuned pair (a slow chorus beat), each with soft upper partials.
            note = np.zeros(n)
            for detune in (-1 / LOOP_SECONDS, 2 / LOOP_SECONDS):
                base = q(f) + detune * (1 + ch)
                for k, a in ((1, 1.0), (2, 0.22), (3, 0.06)):
                    note += w * a * np.sin(2 * np.pi * base * k * t + phase * k)
            out += note * swell(1 + (i + ch) % 3, rng.uniform(0, 2 * np.pi))
        # "Air": noise kept to 1.5–5 kHz. Filtering in the frequency domain over the whole loop
        # makes it circular, so it loops too.
        spectrum = np.fft.rfft(rng.standard_normal(n))
        freqs = np.fft.rfftfreq(n, 1 / RATE)
        band = np.exp(-(((np.log(np.maximum(freqs, 1)) - np.log(2800)) / 0.45) ** 2))
        air = np.fft.irfft(spectrum * band, n)
        air *= swell(2, ch * 1.3) / np.max(np.abs(air))
        out = out / np.max(np.abs(out)) + 0.035 * air
        channels.append(out)
    stereo = np.stack(channels, axis=1)
    stereo = stereo / np.max(np.abs(stereo)) * 10 ** (-14 / 20)
    raw = stereo.astype("<f4").tobytes()
    target = os.path.join(OUT, "ambient_loop.ogg")
    subprocess.run(
        [
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
            "-f", "f32le", "-ar", str(RATE), "-ac", "2", "-i", "-",
            "-c:a", "libvorbis", "-q:a", "3",
            "-metadata", "title=Hearth ambient loop",
            "-metadata", "copyright=CC0, generated by scripts/make-default-sounds.py",
            target,
        ],
        input=raw,
        check=True,
    )


def main():
    os.makedirs(OUT, exist_ok=True)
    ui_sounds()
    ambient()
    for name in sorted(os.listdir(OUT)):
        print(name, os.path.getsize(os.path.join(OUT, name)), "bytes")


if __name__ == "__main__":
    sys.exit(main())
