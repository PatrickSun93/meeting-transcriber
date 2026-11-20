//! Who has the floor: the caller, or the agent while it thinks and speaks,
//! and when the caller takes it back by talking over the agent (barge-in).
//!
//! Pure logic, fed with events and returning what to do, so the rules can
//! be tested without a microphone, a model or a voice.

/// The microphone is looked at in frames of this many seconds for barge-in.
pub const FRAME_SECONDS: f32 = 0.02;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BargeInConfig {
    /// RMS level of a frame that counts as the caller talking. Higher than
    /// the level that starts an utterance: breathing and a quiet room must
    /// not stop the agent.
    pub level: f32,
    /// How many frames in a row must be that loud.
    pub frames: usize,
    pub enabled: bool,
}

impl Default for BargeInConfig {
    fn default() -> Self {
        Self {
            level: 0.02,
            frames: 10, // 200 ms
            enabled: true,
        }
    }
}

/// Counts loud frames in a row.
#[derive(Debug, Clone)]
pub struct VoiceDetector {
    cfg: BargeInConfig,
    run: usize,
}

impl VoiceDetector {
    pub fn new(cfg: BargeInConfig) -> Self {
        Self { cfg, run: 0 }
    }

    /// True on the frame that completes a run of `frames` loud frames.
    pub fn frame(&mut self, rms: f32) -> bool {
        if rms >= self.cfg.level {
            self.run += 1;
            self.run == self.cfg.frames
        } else {
            self.run = 0;
            false
        }
    }

    pub fn reset(&mut self) {
        self.run = 0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The caller has the floor; nothing is being answered.
    Listening,
    /// A turn is being worked on and nothing is heard from the agent yet.
    Thinking,
    /// The agent's voice is playing.
    Speaking,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Answer what the caller said as turn `turn`. `cancel` is the turn
    /// still running, to be stopped first (the caller went on talking).
    StartTurn { turn: u64, cancel: Option<u64> },
    /// The caller talked over turn `turn`: stop its voice and its model.
    Interrupt { turn: u64 },
}

#[derive(Debug)]
pub struct Floor {
    phase: Phase,
    /// The turn being answered (Thinking or Speaking), or the last one.
    turn: u64,
    detector: VoiceDetector,
    enabled: bool,
}

impl Floor {
    pub fn new(cfg: BargeInConfig) -> Self {
        Self {
            phase: Phase::Listening,
            turn: 0,
            detector: VoiceDetector::new(cfg),
            enabled: cfg.enabled,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// The agent says something without being asked (the greeting).
    pub fn agent_starts(&mut self) -> u64 {
        self.turn += 1;
        self.phase = Phase::Thinking;
        self.detector.reset();
        self.turn
    }

    /// The caller finished an utterance and it was understood.
    pub fn caller_said(&mut self) -> Action {
        let cancel = (self.phase != Phase::Listening).then_some(self.turn);
        self.turn += 1;
        self.phase = Phase::Thinking;
        self.detector.reset();
        Action::StartTurn {
            turn: self.turn,
            cancel,
        }
    }

    /// One microphone frame's level. Barge-in while the agent thinks too:
    /// a caller who paused and goes on was not done.
    pub fn mic_frame(&mut self, rms: f32) -> Option<Action> {
        if !self.enabled || self.phase == Phase::Listening {
            return None;
        }
        if self.detector.frame(rms) {
            self.phase = Phase::Listening;
            return Some(Action::Interrupt { turn: self.turn });
        }
        None
    }

    /// The first sound of `turn` was played.
    pub fn voice_started(&mut self, turn: u64) {
        if turn == self.turn && self.phase == Phase::Thinking {
            self.phase = Phase::Speaking;
        }
    }

    /// `turn` said all it had to say (or failed).
    pub fn turn_done(&mut self, turn: u64) {
        if turn == self.turn {
            self.phase = Phase::Listening;
            self.detector.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOUD: f32 = 0.1;
    const QUIET: f32 = 0.005;

    fn floor() -> Floor {
        Floor::new(BargeInConfig {
            level: 0.02,
            frames: 3,
            enabled: true,
        })
    }

    #[test]
    fn a_turn_runs_from_thinking_to_speaking_to_listening() {
        let mut f = floor();
        assert_eq!(f.caller_said(), Action::StartTurn { turn: 1, cancel: None });
        assert_eq!(f.phase(), Phase::Thinking);
        f.voice_started(1);
        assert_eq!(f.phase(), Phase::Speaking);
        f.turn_done(1);
        assert_eq!(f.phase(), Phase::Listening);
    }

    #[test]
    fn talking_over_the_agent_interrupts_it_after_enough_loud_frames() {
        let mut f = floor();
        f.caller_said();
        f.voice_started(1);
        assert_eq!(f.mic_frame(LOUD), None);
        assert_eq!(f.mic_frame(LOUD), None);
        assert_eq!(f.mic_frame(LOUD), Some(Action::Interrupt { turn: 1 }));
        assert_eq!(f.phase(), Phase::Listening);
        // Once is enough: the caller has the floor.
        assert_eq!(f.mic_frame(LOUD), None);
    }

    #[test]
    fn a_short_noise_does_not_interrupt() {
        let mut f = floor();
        f.caller_said();
        f.voice_started(1);
        for _ in 0..10 {
            assert_eq!(f.mic_frame(LOUD), None);
            assert_eq!(f.mic_frame(LOUD), None);
            assert_eq!(f.mic_frame(QUIET), None, "a dip starts the count again");
        }
        assert_eq!(f.phase(), Phase::Speaking);
    }

    #[test]
    fn loud_frames_while_listening_are_just_the_caller_talking() {
        let mut f = floor();
        for _ in 0..10 {
            assert_eq!(f.mic_frame(LOUD), None);
        }
        f.caller_said();
        f.turn_done(1);
        for _ in 0..10 {
            assert_eq!(f.mic_frame(LOUD), None);
        }
    }

    #[test]
    fn going_on_talking_while_the_agent_thinks_cancels_that_turn() {
        let mut f = floor();
        f.caller_said();
        assert_eq!(f.mic_frame(LOUD), None);
        assert_eq!(f.mic_frame(LOUD), None);
        assert_eq!(f.mic_frame(LOUD), Some(Action::Interrupt { turn: 1 }));
        // And if the next utterance comes before anything noticed, it says
        // which turn to stop.
        let mut g = floor();
        g.caller_said();
        assert_eq!(
            g.caller_said(),
            Action::StartTurn {
                turn: 2,
                cancel: Some(1)
            }
        );
    }

    #[test]
    fn events_of_an_old_turn_change_nothing() {
        let mut f = floor();
        f.caller_said();
        f.caller_said();
        f.voice_started(1);
        assert_eq!(f.phase(), Phase::Thinking);
        f.turn_done(1);
        assert_eq!(f.phase(), Phase::Thinking);
        f.voice_started(2);
        assert_eq!(f.phase(), Phase::Speaking);
    }

    #[test]
    fn barge_in_can_be_turned_off() {
        let mut f = Floor::new(BargeInConfig {
            enabled: false,
            ..Default::default()
        });
        f.caller_said();
        f.voice_started(1);
        assert!((0..100).all(|_| f.mic_frame(1.0).is_none()));
    }

    #[test]
    fn the_greeting_can_be_interrupted_too() {
        let mut f = floor();
        assert_eq!(f.agent_starts(), 1);
        f.voice_started(1);
        f.mic_frame(LOUD);
        f.mic_frame(LOUD);
        assert_eq!(f.mic_frame(LOUD), Some(Action::Interrupt { turn: 1 }));
    }
}
