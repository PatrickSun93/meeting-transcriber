//! The sound of a screen recording on Windows, where the microphone and
//! the PC's sound come in packets of their own, each on its own clock:
//! mixed into one track. Each source's samples are laid where they belong
//! on the recording's timeline (its first packet where it arrived, the
//! rest after it), added up, and handed out once every source has sent
//! that far, or a little later anyway, so one that goes quiet or stops
//! does not hold the others back. A source that falls behind what was
//! handed out goes on from there, so it never stays out of step.

use std::collections::VecDeque;

/// Mixes some sources' samples, all at the same rate, into one.
pub struct Mixer {
    /// Each source's next sample's place on the timeline, once it has sent one.
    next: Vec<Option<u64>>,
    /// Samples from `start` on, added up so far.
    mixed: VecDeque<f32>,
    /// The place of `mixed[0]`: everything before it was handed out.
    start: u64,
    /// How far behind the timeline the mix may wait for a quiet source.
    lag: u64,
}

impl Mixer {
    /// For `sources` sources; the mix waits at most `lag` samples for one.
    pub fn new(sources: usize, lag: u64) -> Self {
        Self {
            next: vec![None; sources],
            mixed: VecDeque::new(),
            start: 0,
            lag,
        }
    }

    /// `samples` from `source` came in when the timeline was at `now`.
    pub fn push(&mut self, source: usize, samples: &[f32], now: u64) {
        let Some(slot) = self.next.get_mut(source) else {
            return;
        };
        // The first packet ends about now.
        let next = slot.get_or_insert(now.saturating_sub(samples.len() as u64));
        *next = (*next).max(self.start);
        let at = (*next - self.start) as usize;
        if self.mixed.len() < at + samples.len() {
            self.mixed.resize(at + samples.len(), 0.0);
        }
        for (i, s) in samples.iter().enumerate() {
            self.mixed[at + i] += s;
        }
        *next += samples.len() as u64;
    }

    /// The mix that is ready when the timeline is at `now`, and where it
    /// goes on the timeline: up to where every source that has started has
    /// sent, but never more than `lag` behind `now`. It starts at 0, with
    /// silence until the first source starts.
    pub fn take(&mut self, now: u64) -> (u64, Vec<f32>) {
        let sent = self.next.iter().flatten().copied().min().unwrap_or(0);
        let ready = sent.max(now.saturating_sub(self.lag));
        let count = ready.saturating_sub(self.start) as usize;
        if self.mixed.len() < count {
            self.mixed.resize(count, 0.0);
        }
        let out = self.mixed.drain(..count).map(|s| s.clamp(-1.0, 1.0)).collect();
        let at = self.start;
        self.start += count as u64;
        (at, out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sources_in_step_are_added_up() {
        let mut mixer = Mixer::new(2, 1000);
        mixer.push(0, &[0.25; 100], 100);
        // Only one has sent: the other has not started, so it waits for nobody.
        assert_eq!(mixer.take(100), (0, vec![0.25; 100]));
        mixer.push(1, &[0.5; 100], 200);
        mixer.push(0, &[0.25; 100], 200);
        let (at, mix) = mixer.take(200);
        assert_eq!(at, 100);
        assert_eq!(mix, vec![0.75; 100]);
        // Loud on both is kept in range.
        mixer.push(0, &[0.75; 10], 210);
        mixer.push(1, &[0.75; 10], 210);
        assert_eq!(mixer.take(210).1, vec![1.0; 10]);
    }

    #[test]
    fn the_mix_waits_for_a_slow_source_but_not_for_ever() {
        let mut mixer = Mixer::new(2, 50);
        mixer.push(0, &[0.1; 100], 100);
        mixer.push(1, &[0.2; 40], 100);
        // Source 1 started at 60: ready up to where both have sent, 100.
        let (at, mix) = mixer.take(100);
        assert_eq!(at, 0);
        assert_eq!(mix.len(), 100);
        assert!((mix[0] - 0.1).abs() < 1e-6 && (mix[99] - 0.3).abs() < 1e-6);
        mixer.push(0, &[0.1; 100], 200);
        // Source 1 is quiet: the mix goes on once it is `lag` behind.
        let (at, mix) = mixer.take(200);
        assert_eq!((at, mix.len()), (100, 50));
        // It comes back late: its sound goes on where the mix is, not
        // into what was already handed out, and the mix waits for it again.
        mixer.push(1, &[0.2; 10], 205);
        let (at, mix) = mixer.take(205);
        assert_eq!((at, mix.len()), (150, 10));
        assert!(mix.iter().all(|s| (s - 0.3).abs() < 1e-6));
    }

    #[test]
    fn silence_until_the_first_source_starts() {
        let mut mixer = Mixer::new(2, 50);
        assert_eq!(mixer.take(30), (0, Vec::new()));
        let (at, mix) = mixer.take(80);
        assert_eq!((at, mix), (0, vec![0.0; 30]));
        // The first packet ends now, so it starts 20 back, after the silence.
        mixer.push(1, &[0.5; 20], 100);
        let (at, mix) = mixer.take(100);
        assert_eq!((at, mix.len()), (30, 70));
        assert_eq!(mix[49], 0.0);
        assert_eq!(mix[50], 0.5);
        // A source it does not have is left out.
        mixer.push(7, &[1.0], 100);
        assert_eq!(mixer.take(100), (100, Vec::new()));
    }
}
