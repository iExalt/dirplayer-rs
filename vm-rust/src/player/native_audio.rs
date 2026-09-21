//! Device-free native Director audio built from the same Rodio source path as
//! the production Bevy parity worker.

use std::{
    collections::VecDeque,
    io::Cursor,
    sync::{Arc, Mutex},
};

use rodio::{
    ChannelCount, Decoder, SampleRate, Source,
    mixer::{Mixer, MixerSource, mixer},
    source::UniformSourceIterator,
};

use crate::player::{ScriptError, cast_member::SoundMember};

const OUTPUT_CHANNELS: u16 = 2;
const OUTPUT_RATE: u32 = 48_000;

#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeSoundTransform {
    pub(crate) left_to_left: f32,
    pub(crate) left_to_right: f32,
    pub(crate) right_to_left: f32,
    pub(crate) right_to_right: f32,
}

impl Default for NativeSoundTransform {
    fn default() -> Self {
        Self {
            left_to_left: 1.0,
            left_to_right: 0.0,
            right_to_left: 0.0,
            right_to_right: 1.0,
        }
    }
}

struct PreparedSegment {
    bytes: Arc<[u8]>,
    source_channels: u16,
    volume_gain: f32,
    pan: f64,
    playback_rate: f32,
    remaining: u32,
    source: Option<Box<dyn Source<Item = f32> + Send>>,
}

impl PreparedSegment {
    fn new(
        bytes: Arc<[u8]>,
        source_channels: u16,
        loop_count: i32,
        playback_rate: f32,
        volume: f64,
        pan: f64,
    ) -> Result<Self, ScriptError> {
        let remaining = u32::try_from(loop_count)
            .map_err(|_| ScriptError::new("native audio loop count is invalid".to_owned()))?;
        if remaining == 0 {
            return Err(ScriptError::new(
                "native audio loop count is invalid".to_owned(),
            ));
        }
        let mut segment = Self {
            bytes,
            source_channels,
            volume_gain: (volume / 255.0).clamp(0.0, 1.0) as f32,
            pan,
            playback_rate,
            remaining,
            source: None,
        };
        segment.source = Some(segment.build_source()?);
        Ok(segment)
    }

    fn build_source(&self) -> Result<Box<dyn Source<Item = f32> + Send>, ScriptError> {
        let decoder = Decoder::builder()
            .with_byte_len(self.bytes.len() as u64)
            .with_data(Cursor::new(self.bytes.clone()))
            .build()
            .map_err(|error| ScriptError::new(format!("Failed to decode MP3: {error}")))?;
        // Match the production Bevy/Rodio order exactly: scalar gain is
        // applied before speed conversion and channel/rate normalization.
        // The default Director values (255, 0) bypass all arithmetic here.
        let source: Box<dyn Source<Item = f32> + Send> = if self.volume_gain == 1.0 {
            Box::new(decoder)
        } else {
            Box::new(decoder.amplify(self.volume_gain))
        };
        let source = source.speed(self.playback_rate);
        let source = UniformSourceIterator::new(
            source,
            ChannelCount::new(OUTPUT_CHANNELS).expect("two channels is nonzero"),
            SampleRate::new(OUTPUT_RATE).expect("48 kHz is nonzero"),
        );
        Ok(Box::new(TransformSource {
            inner: Box::new(source),
            transform: native_pan_transform(self.source_channels, self.pan),
            pending_right: None,
        }))
    }

    fn raw(bytes: &[u8]) -> Result<Self, ScriptError> {
        let mut segment = Self {
            bytes: Arc::from(bytes),
            source_channels: OUTPUT_CHANNELS,
            volume_gain: 1.0,
            pan: 0.0,
            playback_rate: 1.0,
            remaining: 1,
            source: None,
        };
        segment.source = Some(segment.build_source()?);
        Ok(segment)
    }

    fn next(&mut self) -> Result<Option<f32>, ScriptError> {
        let Some(source) = self.source.as_mut() else {
            return Ok(None);
        };
        if let Some(sample) = source.next() {
            return Ok(Some(sample));
        }
        self.remaining = self.remaining.saturating_sub(1);
        if self.remaining == 0 {
            self.source = None;
            return Ok(None);
        }
        self.source = Some(self.build_source()?);
        Ok(self.source.as_mut().and_then(|source| source.next()))
    }
}

struct TransformSource {
    inner: Box<dyn Source<Item = f32> + Send>,
    transform: NativeSoundTransform,
    pending_right: Option<f32>,
}

impl Iterator for TransformSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(right) = self.pending_right.take() {
            return Some(right);
        }
        let left = self.inner.next()?;
        let right = self.inner.next().unwrap_or(0.0);
        self.pending_right = Some(left.mul_add(
            self.transform.left_to_right,
            right * self.transform.right_to_right,
        ));
        Some(left.mul_add(
            self.transform.left_to_left,
            right * self.transform.right_to_left,
        ))
    }
}

impl Source for TransformSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(OUTPUT_CHANNELS).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(OUTPUT_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

struct ChannelState {
    current: Option<PreparedSegment>,
    queued: VecDeque<PreparedSegment>,
    pending_error: Option<ScriptError>,
    completed: bool,
}

impl ChannelState {
    fn idle() -> Self {
        Self {
            current: None,
            queued: VecDeque::new(),
            pending_error: None,
            completed: true,
        }
    }

    fn next_sample(&mut self) -> f32 {
        loop {
            let Some(current) = self.current.as_mut() else {
                self.completed = true;
                return 0.0;
            };
            match current.next() {
                Ok(Some(sample)) => return sample,
                Ok(None) => {
                    self.current = self.queued.pop_front();
                    if self.current.is_none() {
                        self.completed = true;
                    }
                }
                Err(error) => {
                    self.pending_error = Some(error);
                    self.current = None;
                    self.queued.clear();
                    self.completed = true;
                    return 0.0;
                }
            }
        }
    }
}

struct ChannelSource {
    state: Arc<Mutex<ChannelState>>,
}

impl Iterator for ChannelSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        Some(
            self.state
                .lock()
                .ok()
                .map_or(0.0, |mut state| state.next_sample()),
        )
    }
}

impl Source for ChannelSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        ChannelCount::new(OUTPUT_CHANNELS).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(OUTPUT_RATE).unwrap()
    }
    fn total_duration(&self) -> Option<std::time::Duration> {
        None
    }
}

pub(crate) struct NativeSoundInstance;

pub(crate) struct NativeAudioState {
    pub(crate) mixer: Mixer,
    pub(crate) output: MixerSource,
    pub(crate) channels: Vec<Arc<Mutex<ChannelState>>>,
    pub(crate) active: Vec<Option<NativeSoundInstance>>,
    pub(crate) queued: Vec<VecDeque<()>>,
}

pub(crate) fn native_sound_transform(
    source_channels: u16,
    volume: f64,
    pan: f64,
) -> NativeSoundTransform {
    let gain = (volume / 255.0).clamp(0.0, 1.0) as f32;
    if pan == 0.0 {
        return NativeSoundTransform {
            left_to_left: gain,
            left_to_right: 0.0,
            right_to_left: 0.0,
            right_to_right: gain,
        };
    }
    let pan = (pan / 100.0).clamp(-1.0, 1.0);
    let theta = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
    let left = theta.cos() as f32 * gain;
    let right = theta.sin() as f32 * gain;
    if source_channels <= 1 {
        NativeSoundTransform {
            left_to_left: left,
            left_to_right: right,
            right_to_left: 0.0,
            right_to_right: 0.0,
        }
    } else {
        NativeSoundTransform {
            left_to_left: gain * ((1.0 - pan).max(0.0) as f32),
            left_to_right: 0.0,
            right_to_left: 0.0,
            right_to_right: gain * ((1.0 + pan).max(0.0) as f32),
        }
    }
}

fn native_pan_transform(source_channels: u16, pan: f64) -> NativeSoundTransform {
    if pan == 0.0 {
        return NativeSoundTransform::default();
    }
    let pan = (pan / 100.0).clamp(-1.0, 1.0);
    let theta = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
    let left = theta.cos() as f32;
    let right = theta.sin() as f32;
    if source_channels <= 1 {
        NativeSoundTransform {
            left_to_left: left,
            left_to_right: right,
            right_to_left: 0.0,
            right_to_right: 0.0,
        }
    } else {
        NativeSoundTransform {
            left_to_left: (1.0 - pan).max(0.0) as f32,
            left_to_right: 0.0,
            right_to_left: 0.0,
            right_to_right: (1.0 + pan).max(0.0) as f32,
        }
    }
}

pub(crate) fn native_num_loops(loop_count: i32) -> Result<u16, ScriptError> {
    if loop_count <= 0 {
        return Err(ScriptError::new(
            "native rate-0 audio requires a positive loop count".to_owned(),
        ));
    }
    Ok(loop_count.saturating_sub(1) as u16)
}

fn native_validate_playback_rate(playback_rate: f32) -> Result<(), ScriptError> {
    if !playback_rate.is_finite() || playback_rate <= 0.0 {
        return Err(ScriptError::new(format!(
            "native playback rate must be finite and positive: {playback_rate}"
        )));
    }
    Ok(())
}

impl NativeAudioState {
    pub(crate) fn new(num_channels: usize) -> Self {
        let (mixer, output) = mixer(
            ChannelCount::new(OUTPUT_CHANNELS).unwrap(),
            SampleRate::new(OUTPUT_RATE).unwrap(),
        );
        let channels: Vec<_> = (0..num_channels)
            .map(|_| Arc::new(Mutex::new(ChannelState::idle())))
            .collect();
        for state in &channels {
            mixer.add(ChannelSource {
                state: state.clone(),
            });
        }
        Self {
            mixer,
            output,
            channels,
            active: (0..num_channels).map(|_| None).collect(),
            queued: (0..num_channels).map(|_| VecDeque::new()).collect(),
        }
    }

    fn prepare_segment(
        &self,
        member: &SoundMember,
        loop_count: i32,
        playback_rate: f32,
        volume: f64,
        pan: f64,
    ) -> Result<PreparedSegment, ScriptError> {
        native_num_loops(loop_count)?;
        native_validate_playback_rate(playback_rate)?;
        if !member.sound.codec().eq_ignore_ascii_case("mp3") {
            return Err(ScriptError::new(format!(
                "native rate-0 audio does not support {} sound members",
                member.sound.codec()
            )));
        }
        PreparedSegment::new(
            Arc::from(member.sound.data()),
            member.info.channels,
            loop_count,
            playback_rate,
            volume,
            pan,
        )
    }

    pub(crate) fn start_mp3(&mut self, channel: usize, bytes: &[u8]) -> Result<(), ScriptError> {
        if channel >= self.channels.len() {
            return Err(ScriptError::new(format!(
                "Invalid sound channel {}",
                channel + 1
            )));
        }
        let segment = PreparedSegment::raw(bytes)?;
        let mut state = self.channels[channel]
            .lock()
            .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
        state.current = Some(segment);
        state.queued.clear();
        state.pending_error = None;
        state.completed = false;
        self.active[channel] = Some(NativeSoundInstance);
        self.queued[channel].clear();
        Ok(())
    }

    pub(crate) fn play_member(
        &mut self,
        channel: usize,
        member: SoundMember,
        loop_count: i32,
        playback_rate: f32,
        volume: f64,
        pan: f64,
    ) -> Result<(), ScriptError> {
        if channel >= self.channels.len() {
            return Err(ScriptError::new(format!(
                "Invalid sound channel {}",
                channel + 1
            )));
        }
        let segment = self.prepare_segment(&member, loop_count, playback_rate, volume, pan)?;
        let mut state = self.channels[channel]
            .lock()
            .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
        state.current = Some(segment);
        state.queued.clear();
        state.pending_error = None;
        state.completed = false;
        self.active[channel] = Some(NativeSoundInstance);
        self.queued[channel].clear();
        Ok(())
    }

    pub(crate) fn play_queue(
        &mut self,
        channel: usize,
        segments: Vec<(SoundMember, i32, f32)>,
        volume: f64,
        pan: f64,
    ) -> Result<(), ScriptError> {
        if channel >= self.channels.len() {
            return Err(ScriptError::new(format!(
                "Invalid sound channel {}",
                channel + 1
            )));
        }
        if segments.is_empty() {
            return Err(ScriptError::new("native audio queue is empty".to_owned()));
        }
        let mut prepared = Vec::with_capacity(segments.len());
        for (member, loop_count, playback_rate) in segments {
            prepared.push(self.prepare_segment(&member, loop_count, playback_rate, volume, pan)?);
        }
        let first = prepared.remove(0);
        let mut state = self.channels[channel]
            .lock()
            .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
        state.current = Some(first);
        state.queued = prepared.into_iter().collect();
        state.pending_error = None;
        state.completed = false;
        self.active[channel] = Some(NativeSoundInstance);
        self.queued[channel] = (0..state.queued.len()).map(|_| ()).collect();
        Ok(())
    }

    pub(crate) fn stop_channel(&mut self, channel: usize) -> Result<(), ScriptError> {
        let Some(state) = self.channels.get(channel) else {
            return Err(ScriptError::new(format!(
                "Invalid sound channel {}",
                channel + 1
            )));
        };
        let mut state = state
            .lock()
            .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
        state.current = None;
        state.queued.clear();
        state.pending_error = None;
        state.completed = true;
        self.active[channel] = None;
        self.queued[channel].clear();
        Ok(())
    }

    pub(crate) fn mix(&mut self, output: &mut [f32]) -> Result<(), ScriptError> {
        for sample in output.iter_mut() {
            *sample = self.output.next().unwrap_or(0.0);
        }
        for (index, state) in self.channels.iter().enumerate() {
            let mut state = state
                .lock()
                .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
            if let Some(error) = state.pending_error.take() {
                return Err(error);
            }
            if state.completed {
                self.active[index] = None;
                self.queued[index].clear();
            }
        }
        Ok(())
    }

    pub(crate) fn activity_snapshot(&self) -> Result<Vec<(bool, usize)>, ScriptError> {
        self.channels
            .iter()
            .map(|state| {
                let state = state.lock().map_err(|_| {
                    ScriptError::new("native audio channel lock poisoned".to_owned())
                })?;
                Ok((
                    state.current.is_some() && !state.completed,
                    state.queued.len(),
                ))
            })
            .collect()
    }

    pub(crate) fn is_busy(&self, channel: usize) -> Result<bool, ScriptError> {
        let state = self.channels.get(channel).ok_or_else(|| {
            ScriptError::new(format!("Invalid sound channel {}", channel + 1))
        })?;
        let state = state
            .lock()
            .map_err(|_| ScriptError::new("native audio channel lock poisoned".to_owned()))?;
        Ok((state.current.is_some() && !state.completed) || !state.queued.is_empty())
    }

    pub(crate) fn stop_all(&mut self) {
        for index in 0..self.channels.len() {
            let _ = self.stop_channel(index);
        }
    }
}
