// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! SoundFont 2 file reading: the RIFF chunks (`sfbk`), the preset and instrument bags with their generators, and the
//! 16-bit sample pool. Every read is bounds checked: a file that does not parse leaves an error, never a panic.
//!
//! The generator opcodes follow the SoundFont 2.04 file layout as FluidSynth and TinySoundFont read it (they agree):
//! instrument 41, keyRange 43, velRange 44, sampleID 53, sampleModes 54, scaleTuning 56, overridingRootKey 58.
// spec: plugins/soundfont-player.md

use std::sync::Arc;

/// Generator opcodes (SoundFont 2.04 §8.1).
pub mod gens {
    pub const START_ADDRS_OFFSET: u16 = 0;
    pub const END_ADDRS_OFFSET: u16 = 1;
    pub const STARTLOOP_ADDRS_OFFSET: u16 = 2;
    pub const ENDLOOP_ADDRS_OFFSET: u16 = 3;
    pub const START_ADDRS_COARSE: u16 = 4;
    pub const MOD_LFO_TO_PITCH: u16 = 5;
    pub const VIB_LFO_TO_PITCH: u16 = 6;
    pub const MOD_ENV_TO_PITCH: u16 = 7;
    pub const INITIAL_FILTER_FC: u16 = 8;
    pub const INITIAL_FILTER_Q: u16 = 9;
    pub const MOD_LFO_TO_FILTER_FC: u16 = 10;
    pub const MOD_ENV_TO_FILTER_FC: u16 = 11;
    pub const END_ADDRS_COARSE: u16 = 12;
    pub const MOD_LFO_TO_VOLUME: u16 = 13;
    pub const CHORUS_SEND: u16 = 15;
    pub const REVERB_SEND: u16 = 16;
    pub const PAN: u16 = 17;
    pub const DELAY_MOD_LFO: u16 = 21;
    pub const FREQ_MOD_LFO: u16 = 22;
    pub const DELAY_VIB_LFO: u16 = 23;
    pub const FREQ_VIB_LFO: u16 = 24;
    pub const DELAY_MOD_ENV: u16 = 25;
    pub const ATTACK_MOD_ENV: u16 = 26;
    pub const HOLD_MOD_ENV: u16 = 27;
    pub const DECAY_MOD_ENV: u16 = 28;
    pub const SUSTAIN_MOD_ENV: u16 = 29;
    pub const RELEASE_MOD_ENV: u16 = 30;
    pub const KEY_TO_MOD_ENV_HOLD: u16 = 31;
    pub const KEY_TO_MOD_ENV_DECAY: u16 = 32;
    pub const DELAY_VOL_ENV: u16 = 33;
    pub const ATTACK_VOL_ENV: u16 = 34;
    pub const HOLD_VOL_ENV: u16 = 35;
    pub const DECAY_VOL_ENV: u16 = 36;
    pub const SUSTAIN_VOL_ENV: u16 = 37;
    pub const RELEASE_VOL_ENV: u16 = 38;
    pub const KEY_TO_VOL_ENV_HOLD: u16 = 39;
    pub const KEY_TO_VOL_ENV_DECAY: u16 = 40;
    pub const INSTRUMENT: u16 = 41;
    pub const KEY_RANGE: u16 = 43;
    pub const VEL_RANGE: u16 = 44;
    pub const STARTLOOP_ADDRS_COARSE: u16 = 45;
    pub const KEYNUM: u16 = 46;
    pub const VELOCITY: u16 = 47;
    pub const INITIAL_ATTENUATION: u16 = 48;
    pub const ENDLOOP_ADDRS_COARSE: u16 = 50;
    pub const COARSE_TUNE: u16 = 51;
    pub const FINE_TUNE: u16 = 52;
    pub const SAMPLE_ID: u16 = 53;
    pub const SAMPLE_MODES: u16 = 54;
    pub const SCALE_TUNING: u16 = 56;
    pub const EXCLUSIVE_CLASS: u16 = 57;
    pub const OVERRIDING_ROOT_KEY: u16 = 58;
}

/// A sample's header (`shdr` record).
#[derive(Clone, Debug, PartialEq)]
pub struct SampleHeader {
    pub name: String,
    /// Frames into the sample pool (16-bit words), end exclusive.
    pub start: usize,
    pub end: usize,
    /// Loop frames, start inclusive, end exclusive (`loopEnd` is one past, as the file stores it).
    pub loop_start: usize,
    pub loop_end: usize,
    pub rate: u32,
    /// The sample's root key (MIDI note), 0..127.
    pub pitch: u8,
    /// Pitch correction in cents (-50..50).
    pub correction: i8,
    /// 0 mono, 1 left, 2 right, 4 linked, 8 ROM.
    pub kind: u16,
}

/// One zone's generators as the file stores them (amounts are the raw `short` / packed values).
pub type Gens = Vec<(u16, i32)>;

/// A bag (zone): its generators.
#[derive(Clone, Debug, Default)]
pub struct Zone {
    pub gens: Gens,
}

impl Zone {
    /// The amount of generator `op` in the zone, if it sets one.
    pub fn field(&self, op: u16) -> Option<i32> {
        self.gens.iter().find(|(o, _)| *o == op).map(|(_, v)| *v)
    }

    /// The zone's key range, as (lo, hi) (0..127 each), when it sets one.
    pub fn key_range(&self) -> Option<(u8, u8)> {
        self.field(gens::KEY_RANGE).map(|v| unpack_range(v))
    }

    /// The zone's velocity range, as (lo, hi).
    pub fn vel_range(&self) -> Option<(u8, u8)> {
        self.field(gens::VEL_RANGE).map(|v| unpack_range(v))
    }
}

/// Unpacks a keyRange / velRange amount: low byte, high byte.
fn unpack_range(v: i32) -> (u8, u8) {
    (v as u16 as u8, (v as u16 >> 8) as u8)
}

/// A preset (`phdr` record): bank and program number with its preset zones.
#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    /// Zones `bag_lo..bag_hi` of the file's preset bag list.
    pub zones: Vec<Zone>,
}

/// An instrument (`inst` record) with its instrument zones.
#[derive(Clone, Debug)]
pub struct Instrument {
    pub name: String,
    pub zones: Vec<Zone>,
}

/// A parsed SoundFont: its name, the presets a player lists, the instruments and samples behind them, and the shared
/// 16-bit sample pool (`sdta`'s `smpl`).
pub struct SoundFont {
    pub name: String,
    pub presets: Vec<Preset>,
    pub instruments: Vec<Instrument>,
    pub samples: Vec<SampleHeader>,
    pub data: Arc<Vec<i16>>,
}

impl SoundFont {
    /// The preset matching a bank / program pair, if the file has one.
    pub fn find(&self, bank: u16, program: u16) -> Option<usize> {
        self.presets.iter().position(|p| p.bank == bank && p.program == program)
    }
}

/// An error while reading a soundfont.
#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Checked reads over a byte slice.
struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }

    fn at(&self, pos: usize) -> Result<Reader<'a>, Error> {
        if pos > self.b.len() {
            return Err(Error("chunk past the end of the file".into()));
        }
        Ok(Reader { b: &self.b[pos..] })
    }

    fn len(&self) -> usize {
        self.b.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.b.len() < n {
            return Err(Error(format!("unexpected end of the file: wanted {n}, had {}", self.b.len())));
        }
        let (a, rest) = self.b.split_at(n);
        self.b = rest;
        Ok(a)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let a = self.take(2)?;
        Ok(u16::from_le_bytes([a[0], a[1]]))
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn i16(&mut self) -> Result<i16, Error> {
        Ok(self.u16()? as i16)
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let a = self.take(4)?;
        Ok(u32::from_le_bytes([a[0], a[1], a[2], a[3]]))
    }

    /// A fixed-width string field: bytes to the first NUL, lossy UTF-8.
    fn text(&mut self, n: usize) -> Result<String, Error> {
        let a = self.take(n)?;
        let end = a.iter().position(|&b| b == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&a[..end]).into_owned())
    }

    /// A RIFF chunk header: its id and the length of its body. The body follows (padded to even in the file, which the
    /// length already covers for the chunks this format reads).
    fn chunk(&mut self) -> Result<([u8; 4], Reader<'a>), Error> {
        let mut id = [0u8; 4];
        id.copy_from_slice(self.take(4)?);
        let len = self.u32()? as usize;
        let body = self.take(len.min(self.b.len()))?;
        Ok((id, Reader::new(body)))
    }

    fn fourcc(&mut self) -> Result<[u8; 4], Error> {
        let mut id = [0u8; 4];
        id.copy_from_slice(self.take(4)?);
        Ok(id)
    }

    /// A LIST chunk: its type and body (the type is the body's first four bytes).
    fn list(&mut self) -> Result<([u8; 4], Reader<'a>), Error> {
        let (id, mut body) = self.chunk()?;
        if &id != b"LIST" {
            return Err(Error(format!("expected LIST, found {}", String::from_utf8_lossy(&id))));
        }
        let ty = body.fourcc()?;
        Ok((ty, body))
    }
}

/// Parses a SoundFont 2 file (`sfbk` RIFF) from its bytes.
pub fn parse(bytes: &[u8]) -> Result<SoundFont, Error> {
    let mut top = Reader::new(bytes);
    if top.fourcc()? != *b"RIFF" {
        return Err(Error("not a RIFF file".into()));
    }
    let _len = top.u32()?;
    if top.fourcc()? != *b"sfbk" {
        return Err(Error("not a SoundFont (no sfbk form)".into()));
    }

    // INFO: the bank's name (INAM).
    let (info_ty, mut info) = top.list()?;
    if &info_ty != b"INFO" {
        return Err(Error("no INFO list".into()));
    }
    let mut name = String::new();
    while info.len() >= 8 {
        let (id, mut body) = info.chunk()?;
        if &id == b"IFIL" {
            // A SF2 v2 file: anything below 2.00 is not readable.
            let ver = body.u16().unwrap_or(0);
            if ver < 2 {
                return Err(Error(format!("SoundFont version {ver}.x is not supported (2.x needed)")));
            }
        } else if &id == b"INAM" {
            name = String::from_utf8_lossy(body.b).trim_end_matches('\0').to_string();
        }
    }

    // sdta: the 16-bit sample pool.
    let (sdta_ty, mut sdta) = top.list()?;
    if &sdta_ty != b"sdta" {
        return Err(Error("no sdta list".into()));
    }
    let mut data: Vec<i16> = Vec::new();
    while sdta.len() >= 8 {
        let (id, body) = sdta.chunk()?;
        if &id == b"smpl" {
            let n = body.b.len() / 2;
            data.reserve(n);
            let mut r = body;
            for _ in 0..n {
                data.push(r.i16()?);
            }
        }
    }

    // pdta: the preset and instrument tables.
    let (pdta_ty, mut pdta) = top.list()?;
    if &pdta_ty != b"pdta" {
        return Err(Error("no pdta list".into()));
    }
    let mut phdr: Vec<(String, u16, u16, usize)> = Vec::new();
    let mut pbag: Vec<(usize, usize)> = Vec::new();
    let mut pgen: Vec<(u16, i32)> = Vec::new();
    let mut inst: Vec<(String, usize)> = Vec::new();
    let mut ibag: Vec<(usize, usize)> = Vec::new();
    let mut igen: Vec<(u16, i32)> = Vec::new();
    let mut shdr: Vec<SampleHeader> = Vec::new();
    while pdta.len() >= 8 {
        let (id, body) = pdta.chunk()?;
        match &id {
            b"phdr" => {
                let mut r = body;
                while r.len() >= 38 {
                    let n = r.text(20)?;
                    let program = r.u16()?;
                    let bank = r.u16()?;
                    let bag = r.u16()? as usize;
                    let _library = r.u32()?;
                    let _genre = r.u32()?;
                    let _morph = r.u32()?;
                    phdr.push((n, program, bank, bag));
                }
            }
            b"pbag" => {
                let mut r = body;
                while r.len() >= 4 {
                    let g = r.u16()? as usize;
                    let m = r.u16()? as usize;
                    pbag.push((g, m));
                }
            }
            b"pgen" => {
                let mut r = body;
                while r.len() >= 4 {
                    let op = r.u16()?;
                    let amount = r.i16()?;
                    pgen.push((op, i32::from(amount)));
                }
            }
            b"inst" => {
                let mut r = body;
                while r.len() >= 22 {
                    let n = r.text(20)?;
                    let bag = r.u16()? as usize;
                    inst.push((n, bag));
                }
            }
            b"ibag" => {
                let mut r = body;
                while r.len() >= 4 {
                    let g = r.u16()? as usize;
                    let m = r.u16()? as usize;
                    ibag.push((g, m));
                }
            }
            b"igen" => {
                let mut r = body;
                while r.len() >= 4 {
                    let op = r.u16()?;
                    let amount = r.i16()?;
                    igen.push((op, i32::from(amount)));
                }
            }
            b"shdr" => {
                let mut r = body;
                while r.len() >= 46 {
                    let n = r.text(20)?;
                    let start = r.u32()? as usize;
                    let end = r.u32()? as usize;
                    let loop_start = r.u32()? as usize;
                    let loop_end = r.u32()? as usize;
                    let rate = r.u32()?;
                    let pitch = r.u8()?;
                    let correction = r.u8()? as i8;
                    let _link = r.u16()?;
                    let kind = r.u16()?;
                    shdr.push(SampleHeader { name: n, start, end, loop_start, loop_end, rate, pitch, correction, kind });
                }
            }
            _ => {}
        }
    }

    // The last records are terminators (EOP / EOS); they close their bag ranges.
    if phdr.len() < 2 || pbag.is_empty() || inst.is_empty() || ibag.is_empty() || shdr.is_empty() {
        return Err(Error("the file's tables are empty".into()));
    }
    phdr.pop();
    inst.pop();
    shdr.pop();

    // Presets: the bag ranges from the bag index of each record to the next's.
    let bag_of = |bags: &[(usize, usize)], i: usize| bags.get(i).map(|b| b.0).unwrap_or(usize::MAX);
    let mut presets = Vec::new();
    for (i, (n, program, bank, bag)) in phdr.iter().enumerate() {
        let lo = *bag;
        let hi = bag_of(&pbag, i + 1);
        let zones = read_zones(lo, hi, &pbag, &pgen)?;
        presets.push(Preset { name: n.clone(), bank: *bank, program: *program, zones });
    }

    // Instruments likewise.
    let mut instruments = Vec::new();
    for (i, (n, bag)) in inst.iter().enumerate() {
        let lo = *bag;
        let hi = bag_of(&ibag, i + 1);
        let zones = read_zones(lo, hi, &ibag, &igen)?;
        instruments.push(Instrument { name: n.clone(), zones });
    }

    if name.is_empty() {
        name = "SoundFont".into();
    }
    Ok(SoundFont { name, presets, instruments, samples: shdr, data: Arc::new(data) })
}

/// The zones of one bag range: generators `bags[i].0 .. bags[i+1].0` from the generator list.
fn read_zones(lo: usize, hi: usize, bags: &[(usize, usize)], gens: &[(u16, i32)]) -> Result<Vec<Zone>, Error> {
    if lo == usize::MAX {
        return Ok(Vec::new());
    }
    let mut zones = Vec::new();
    for i in lo..hi.min(bags.len().max(lo + 1)) {
        let g_lo = bags.get(i).map(|b| b.0).unwrap_or(gens.len());
        let g_hi = bags.get(i + 1).map(|b| b.0).unwrap_or(gens.len()).max(g_lo);
        let gens = &gens[g_lo.min(gens.len())..g_hi.min(gens.len())];
        zones.push(Zone { gens: gens.to_vec() });
    }
    Ok(zones)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal SoundFont 2 file in memory: one preset ("Test"), one instrument, one mono sample of `n`
    /// frames (a ramp), a key range of 0..127 and a volEnv attack of 100 timecents on the instrument zone.
    fn build(n: usize) -> Vec<u8> {
        let mut b = Vec::new();
        let mut list = |ty: &[u8; 4], body: &mut Vec<u8>| {
            let mut l = Vec::new();
            l.extend_from_slice(ty);
            l.append(body);
            while l.len() % 2 == 1 {
                l.push(0);
            }
            b.extend_from_slice(b"LIST");
            b.extend_from_slice(&(l.len() as u32).to_le_bytes());
            b.extend_from_slice(&l);
        };

        let mut info = Vec::new();
        info.extend_from_slice(b"ifil");
        info.extend_from_slice(&4u32.to_le_bytes());
        info.extend_from_slice(&2u16.to_le_bytes());
        info.extend_from_slice(&0u16.to_le_bytes());
        info.extend_from_slice(b"INAM");
        info.extend_from_slice(&8u32.to_le_bytes());
        info.extend_from_slice(b"Test\0\0\0\0");
        list(b"INFO", &mut info);

        let mut sdta = Vec::new();
        sdta.extend_from_slice(b"smpl");
        sdta.extend_from_slice(&((n * 2) as u32).to_le_bytes());
        for i in 0..n {
            sdta.extend_from_slice(&((i as i16).to_le_bytes()));
        }
        list(b"sdta", &mut sdta);

        // pdta. One preset: bag 0..1 with no generators (the whole instrument, no key filter). One instrument: bag
        // 0..1 with keyRange 0..127 and attackVolEnv 100. One sample: frames 0..n.
        let mut pdta = Vec::new();
        let put = |p: &mut Vec<u8>, id: &[u8; 4], body: &[u8]| {
            p.extend_from_slice(id);
            p.extend_from_slice(&(body.len() as u32).to_le_bytes());
            p.extend_from_slice(body);
        };
        let mut rec = [0u8; 38];
        rec[..6].copy_from_slice(b"Test\0\0");
        rec[20..22].copy_from_slice(&0u16.to_le_bytes()); // program 0
        rec[22..24].copy_from_slice(&0u16.to_le_bytes()); // bank 0
        rec[24..26].copy_from_slice(&0u16.to_le_bytes()); // bag 0
        let mut eop = [0u8; 38];
        eop[24..26].copy_from_slice(&1u16.to_le_bytes()); // EOP: bag 1
        put(&mut pdta, b"phdr", &[rec.as_slice(), eop.as_slice()].concat());
        put(&mut pdta, b"pbag", &[0u16.to_le_bytes(), 0u16.to_le_bytes(), 1u16.to_le_bytes(), 0u16.to_le_bytes()].concat());
        put(&mut pdta, b"pgen", &[]);
        let mut irec = [0u8; 22];
        irec[..6].copy_from_slice(b"Inst\0\0");
        irec[20..22].copy_from_slice(&0u16.to_le_bytes());
        let mut eos = [0u8; 22];
        eos[20..22].copy_from_slice(&1u16.to_le_bytes());
        put(&mut pdta, b"inst", &[irec.as_slice(), eos.as_slice()].concat());
        put(&mut pdta, b"ibag", &[0u16.to_le_bytes(), 0u16.to_le_bytes(), 2u16.to_le_bytes(), 0u16.to_le_bytes()].concat());
        // igen: keyRange 0..127, attackVolEnv 100.
        put(&mut pdta, b"igen", &[43u16.to_le_bytes(), (0u8 as u16 | (127u16 << 8)).to_le_bytes(), 34u16.to_le_bytes(), 100i16.to_le_bytes()].concat());
        let mut srec = [0u8; 46];
        srec[..6].copy_from_slice(b"Samp\0\0");
        srec[20..24].copy_from_slice(&0u32.to_le_bytes());
        srec[24..28].copy_from_slice(&(n as u32).to_le_bytes());
        srec[28..32].copy_from_slice(&0u32.to_le_bytes());
        srec[32..36].copy_from_slice(&(n as u32).to_le_bytes());
        srec[36..40].copy_from_slice(&44100u32.to_le_bytes());
        srec[40] = 60; // root key
        let mut eos2 = [0u8; 46];
        eos2[20..24].copy_from_slice(&(n as u32).to_le_bytes());
        put(&mut pdta, b"shdr", &[srec.as_slice(), eos2.as_slice()].concat());
        list(b"pdta", &mut pdta);

        let len = (b.len() + 4) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(b"sfbk");
        out.extend_from_slice(&b);
        out
    }

    #[test]
    fn a_small_file_parses() {
        let f = parse(&build(1000)).unwrap();
        assert_eq!(f.name, "Test");
        assert_eq!(f.presets.len(), 1);
        assert_eq!(f.presets[0].name, "Test");
        assert_eq!((f.presets[0].bank, f.presets[0].program), (0, 0));
        assert_eq!(f.instruments.len(), 1);
        assert_eq!(f.samples.len(), 1);
        assert_eq!(f.samples[0].rate, 44100);
        assert_eq!(f.samples[0].pitch, 60);
        assert_eq!(f.data.len(), 1000);
        assert_eq!(f.data[5], 5);
        assert_eq!(f.find(0, 0), Some(0));
        assert_eq!(f.find(0, 1), None);
    }

    #[test]
    fn generators_and_ranges_arrive() {
        let f = parse(&build(10)).unwrap();
        let iz = &f.instruments[0].zones[0];
        assert_eq!(iz.key_range(), Some((0, 127)));
        assert_eq!(iz.field(gens::ATTACK_VOL_ENV), Some(100));
    }

    #[test]
    fn junk_is_rejected() {
        assert!(parse(b"not a riff").is_err());
        let mut b = build(10);
        b[8..12].copy_from_slice(b"XXXX");
        assert!(parse(&b).is_err());
    }
}
