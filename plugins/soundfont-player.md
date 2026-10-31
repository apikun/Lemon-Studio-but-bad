# Soundfont Player

Lemon Studio's SoundFont 2 player: an instrument plugin (VST3 and CLAP, and compiled into Lemon Studio
itself) that loads `.sf2` files and plays their presets with the original Soundfont Player's layout.

## 1. The file

`plugins/soundfont-player` is one crate: `sf2` parses the file, `dsp` holds the voices, `fx` the built-in
reverb and chorus, `params` the parameters and `view`/`editor` the editor.

The parser (`sf2.rs`) reads the whole RIFF (`sfbk`) with bounds-checked reads: INFO (the name, and the
`ifil` version — below 2.00 is refused), `sdta`'s 16-bit sample pool, and `pdta`'s preset, instrument and
sample tables. Preset and instrument zones are read as bag ranges; a zone's generators follow the
SoundFont 2.04 layout as FluidSynth and TinySoundFont read it (instrument 41, keyRange 43, velRange 44,
sampleID 53, sampleModes 54, scaleTuning 56, overridingRootKey 58). A preset's zones are flattened over
their instruments into regions (`dsp::regions`) when the preset is taken, so a note starts without file
work; the ranges of the preset zone and the instrument zone are both applied (narrowed), and the
instrument zone's generators win over the preset zone's.

## 2. The sound (`dsp.rs`)

A voice is one region: the sample read from the pool with linear interpolation at
`rate · 2^((key − root) · scaleTuning/100 + coarse + fine/100 + bend/100)`, looped per the zone's
`sampleModes` (1 loops till release, 3 loops always; a released voice plays on to the end), panned at equal
power (the file's −500..500; a stereo pair's side hard-pans when the zone sets no pan of its own).

- The **volume envelope** is the file's `volEnv` with the key scaling of hold and decay; the sustain is
  centibels of attenuation. The release falls linearly from where the note left it.
- The **filter** is a two-pole low-pass (Chamberlin) per side, at the Misc CUT knob's frequency moved by
  the modulation envelope and the LFO; the file's own `modEnvToFilterFc` wins over the MOD knob's depth
  when the zone sets one. The file's `initialFilterQ` is the resonance (its quarter-dB units as dB).
- **Envelope 2** (the filter's modulation envelope) is the player's: the A / D / S / R faders, over the
  file's delay. The editor syncs the faders to each preset's `modEnv` when it loads or switches.
- **LFO 2** is the mod LFO: the player's delay and speed when the file sets none, its depth to the pitch
  the file's `modLfoToPitch` (the AMT knob syncs to it at a load), to the filter the file's own depth, and
  to the volume the file's tremolo. The file's vibrato LFO runs beside it.
- A voice's **reverb and chorus sends** leave as their own mono sums, weighted by the zone's
  `reverbEffectsSend` / `chorusEffectsSend` (a zone that sets none sends in full), so the effects hear the
  voices as they sound.
- Exclusive classes choke; the voice cap is 64, the quietest releasing voice is stolen first.

## 3. The effects (`fx.rs`)

The BUILT-IN reverb (a small plate: four combs and two all-passes a side, Freeverb's tunings scaled to the
rate) and chorus (two lines swept a quarter turn apart) take their send sums; the LEVEL knobs set how much
of the send comes back wet, and a switch at 0 costs nothing.

## 4. The editor (`view.rs`, `editor.rs`)

The editor is the original's layout in the Lemon plug look, 560 × 501: the top bar (open a soundfont, its
name, the tag's details menu), the patch row (the preset's `[bank:program]: name`, the previous / next
arrows), the REVERB and CHORUS panels (SEND TO, BUILT-IN, LEVEL), Envelope 2's faders, LFO 2's and Misc's
dials, the gear's options menu (load, reset controls, about) and the wordmark.

Knobs and faders drag with the cursor held (Ctrl fine), the wheel steps 1/128, Alt or a middle click
resets, and a right click opens the parameter menu (reset, copy / paste, type in, create automation clip
when Lemon Studio hosts). The patch box opens the preset list; a load or a preset switch moves Envelope 2,
LFO 2 and Misc to the preset's first zone's values, as the original does at a load. A soundfont dropped on
the editor (macOS) loads. Edits show and sound at once (`os_plugkit::vals`).

## 5. Parameters and state

Every control is a host-visible parameter (`preset`, the two sends' level / built-in / send-to, the four
Envelope 2 faders, the three LFO 2 knobs and the two Misc knobs) with the patch name on the preset's value
text. The loaded file's path rides in the persisted fields (as Lemon Sampler's sample path does); a
restored state loads it in `initialize`. The plugin ids are `org.openstudio.soundfont-player` (CLAP) and
`OpenStudioSfPlay` (VST3).
