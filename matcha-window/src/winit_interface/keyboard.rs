//! winit's keyboard vocabulary, mapped onto the W3C one this crate speaks.
//!
//! Both sides are encodings of the same specification — [UI Events
//! KeyboardEvent][spec] — so 305 of winit's 306 `NamedKey` variants and 191 of
//! its 194 `KeyCode` variants share a name with their `keyboard_types`
//! counterpart, and the mapping below is almost entirely `X => X`. It is
//! spelled out rather than transmuted because the two enums are laid out
//! independently and neither promises a discriminant.
//!
//! The exceptions are all documented by winit itself, which lists where it
//! departs from the spec:
//!
//! - `KeyCode::SuperLeft`/`SuperRight` are the spec's `MetaLeft`/`MetaRight`.
//! - `KeyCode::Meta` is the spec's `Super` (a legacy key; deprecated in
//!   `keyboard_types`, hence the `allow` below).
//! - `NamedKey::Space` has no spec counterpart: the spec reports the space bar
//!   as the character `" "`, which is what this maps it to. A widget therefore
//!   handles space in its `Key::Character` arm, alongside every other
//!   text-producing key, instead of special-casing it.
//!
//! Both winit enums are `#[non_exhaustive]`, so each match ends in a catch-all
//! that degrades to `Unidentified` — a winit upgrade that adds keys cannot
//! break this crate's build, it just leaves the new keys unnamed until someone
//! adds an arm.
//!
//! [spec]: https://w3c.github.io/uievents-key/

#![allow(deprecated)] // `Code::Super` / `NamedKey::Hyper` are legacy in the spec.

use keyboard_types::{Code, Key, Location, Modifiers, NamedKey};
use winit::keyboard as w;

/// The logical key: what the keystroke *means* under the current layout.
pub(super) fn map_key(key: w::Key) -> Key {
    match key {
        w::Key::Character(s) => Key::Character(s.to_string()),
        w::Key::Named(named) => map_named_key(named),
        // winit reports a dead key's own text separately; the spec has a single
        // `Dead` value and puts the text in the composition events instead.
        w::Key::Dead(_) => Key::Named(NamedKey::Dead),
        w::Key::Unidentified(_) => Key::Named(NamedKey::Unidentified),
    }
}

fn map_named_key(named: w::NamedKey) -> Key {
    match named {
        w::NamedKey::Space => Key::Character(" ".to_string()),
        w::NamedKey::Alt => Key::Named(NamedKey::Alt),
        w::NamedKey::AltGraph => Key::Named(NamedKey::AltGraph),
        w::NamedKey::CapsLock => Key::Named(NamedKey::CapsLock),
        w::NamedKey::Control => Key::Named(NamedKey::Control),
        w::NamedKey::Fn => Key::Named(NamedKey::Fn),
        w::NamedKey::FnLock => Key::Named(NamedKey::FnLock),
        w::NamedKey::NumLock => Key::Named(NamedKey::NumLock),
        w::NamedKey::ScrollLock => Key::Named(NamedKey::ScrollLock),
        w::NamedKey::Shift => Key::Named(NamedKey::Shift),
        w::NamedKey::Symbol => Key::Named(NamedKey::Symbol),
        w::NamedKey::SymbolLock => Key::Named(NamedKey::SymbolLock),
        w::NamedKey::Meta => Key::Named(NamedKey::Meta),
        w::NamedKey::Hyper => Key::Named(NamedKey::Hyper),
        w::NamedKey::Super => Key::Named(NamedKey::Super),
        w::NamedKey::Enter => Key::Named(NamedKey::Enter),
        w::NamedKey::Tab => Key::Named(NamedKey::Tab),
        w::NamedKey::ArrowDown => Key::Named(NamedKey::ArrowDown),
        w::NamedKey::ArrowLeft => Key::Named(NamedKey::ArrowLeft),
        w::NamedKey::ArrowRight => Key::Named(NamedKey::ArrowRight),
        w::NamedKey::ArrowUp => Key::Named(NamedKey::ArrowUp),
        w::NamedKey::End => Key::Named(NamedKey::End),
        w::NamedKey::Home => Key::Named(NamedKey::Home),
        w::NamedKey::PageDown => Key::Named(NamedKey::PageDown),
        w::NamedKey::PageUp => Key::Named(NamedKey::PageUp),
        w::NamedKey::Backspace => Key::Named(NamedKey::Backspace),
        w::NamedKey::Clear => Key::Named(NamedKey::Clear),
        w::NamedKey::Copy => Key::Named(NamedKey::Copy),
        w::NamedKey::CrSel => Key::Named(NamedKey::CrSel),
        w::NamedKey::Cut => Key::Named(NamedKey::Cut),
        w::NamedKey::Delete => Key::Named(NamedKey::Delete),
        w::NamedKey::EraseEof => Key::Named(NamedKey::EraseEof),
        w::NamedKey::ExSel => Key::Named(NamedKey::ExSel),
        w::NamedKey::Insert => Key::Named(NamedKey::Insert),
        w::NamedKey::Paste => Key::Named(NamedKey::Paste),
        w::NamedKey::Redo => Key::Named(NamedKey::Redo),
        w::NamedKey::Undo => Key::Named(NamedKey::Undo),
        w::NamedKey::Accept => Key::Named(NamedKey::Accept),
        w::NamedKey::Again => Key::Named(NamedKey::Again),
        w::NamedKey::Attn => Key::Named(NamedKey::Attn),
        w::NamedKey::Cancel => Key::Named(NamedKey::Cancel),
        w::NamedKey::ContextMenu => Key::Named(NamedKey::ContextMenu),
        w::NamedKey::Escape => Key::Named(NamedKey::Escape),
        w::NamedKey::Execute => Key::Named(NamedKey::Execute),
        w::NamedKey::Find => Key::Named(NamedKey::Find),
        w::NamedKey::Help => Key::Named(NamedKey::Help),
        w::NamedKey::Pause => Key::Named(NamedKey::Pause),
        w::NamedKey::Play => Key::Named(NamedKey::Play),
        w::NamedKey::Props => Key::Named(NamedKey::Props),
        w::NamedKey::Select => Key::Named(NamedKey::Select),
        w::NamedKey::ZoomIn => Key::Named(NamedKey::ZoomIn),
        w::NamedKey::ZoomOut => Key::Named(NamedKey::ZoomOut),
        w::NamedKey::BrightnessDown => Key::Named(NamedKey::BrightnessDown),
        w::NamedKey::BrightnessUp => Key::Named(NamedKey::BrightnessUp),
        w::NamedKey::Eject => Key::Named(NamedKey::Eject),
        w::NamedKey::LogOff => Key::Named(NamedKey::LogOff),
        w::NamedKey::Power => Key::Named(NamedKey::Power),
        w::NamedKey::PowerOff => Key::Named(NamedKey::PowerOff),
        w::NamedKey::PrintScreen => Key::Named(NamedKey::PrintScreen),
        w::NamedKey::Hibernate => Key::Named(NamedKey::Hibernate),
        w::NamedKey::Standby => Key::Named(NamedKey::Standby),
        w::NamedKey::WakeUp => Key::Named(NamedKey::WakeUp),
        w::NamedKey::AllCandidates => Key::Named(NamedKey::AllCandidates),
        w::NamedKey::Alphanumeric => Key::Named(NamedKey::Alphanumeric),
        w::NamedKey::CodeInput => Key::Named(NamedKey::CodeInput),
        w::NamedKey::Compose => Key::Named(NamedKey::Compose),
        w::NamedKey::Convert => Key::Named(NamedKey::Convert),
        w::NamedKey::FinalMode => Key::Named(NamedKey::FinalMode),
        w::NamedKey::GroupFirst => Key::Named(NamedKey::GroupFirst),
        w::NamedKey::GroupLast => Key::Named(NamedKey::GroupLast),
        w::NamedKey::GroupNext => Key::Named(NamedKey::GroupNext),
        w::NamedKey::GroupPrevious => Key::Named(NamedKey::GroupPrevious),
        w::NamedKey::ModeChange => Key::Named(NamedKey::ModeChange),
        w::NamedKey::NextCandidate => Key::Named(NamedKey::NextCandidate),
        w::NamedKey::NonConvert => Key::Named(NamedKey::NonConvert),
        w::NamedKey::PreviousCandidate => Key::Named(NamedKey::PreviousCandidate),
        w::NamedKey::Process => Key::Named(NamedKey::Process),
        w::NamedKey::SingleCandidate => Key::Named(NamedKey::SingleCandidate),
        w::NamedKey::HangulMode => Key::Named(NamedKey::HangulMode),
        w::NamedKey::HanjaMode => Key::Named(NamedKey::HanjaMode),
        w::NamedKey::JunjaMode => Key::Named(NamedKey::JunjaMode),
        w::NamedKey::Eisu => Key::Named(NamedKey::Eisu),
        w::NamedKey::Hankaku => Key::Named(NamedKey::Hankaku),
        w::NamedKey::Hiragana => Key::Named(NamedKey::Hiragana),
        w::NamedKey::HiraganaKatakana => Key::Named(NamedKey::HiraganaKatakana),
        w::NamedKey::KanaMode => Key::Named(NamedKey::KanaMode),
        w::NamedKey::KanjiMode => Key::Named(NamedKey::KanjiMode),
        w::NamedKey::Katakana => Key::Named(NamedKey::Katakana),
        w::NamedKey::Romaji => Key::Named(NamedKey::Romaji),
        w::NamedKey::Zenkaku => Key::Named(NamedKey::Zenkaku),
        w::NamedKey::ZenkakuHankaku => Key::Named(NamedKey::ZenkakuHankaku),
        w::NamedKey::Soft1 => Key::Named(NamedKey::Soft1),
        w::NamedKey::Soft2 => Key::Named(NamedKey::Soft2),
        w::NamedKey::Soft3 => Key::Named(NamedKey::Soft3),
        w::NamedKey::Soft4 => Key::Named(NamedKey::Soft4),
        w::NamedKey::ChannelDown => Key::Named(NamedKey::ChannelDown),
        w::NamedKey::ChannelUp => Key::Named(NamedKey::ChannelUp),
        w::NamedKey::Close => Key::Named(NamedKey::Close),
        w::NamedKey::MailForward => Key::Named(NamedKey::MailForward),
        w::NamedKey::MailReply => Key::Named(NamedKey::MailReply),
        w::NamedKey::MailSend => Key::Named(NamedKey::MailSend),
        w::NamedKey::MediaClose => Key::Named(NamedKey::MediaClose),
        w::NamedKey::MediaFastForward => Key::Named(NamedKey::MediaFastForward),
        w::NamedKey::MediaPause => Key::Named(NamedKey::MediaPause),
        w::NamedKey::MediaPlay => Key::Named(NamedKey::MediaPlay),
        w::NamedKey::MediaPlayPause => Key::Named(NamedKey::MediaPlayPause),
        w::NamedKey::MediaRecord => Key::Named(NamedKey::MediaRecord),
        w::NamedKey::MediaRewind => Key::Named(NamedKey::MediaRewind),
        w::NamedKey::MediaStop => Key::Named(NamedKey::MediaStop),
        w::NamedKey::MediaTrackNext => Key::Named(NamedKey::MediaTrackNext),
        w::NamedKey::MediaTrackPrevious => Key::Named(NamedKey::MediaTrackPrevious),
        w::NamedKey::New => Key::Named(NamedKey::New),
        w::NamedKey::Open => Key::Named(NamedKey::Open),
        w::NamedKey::Print => Key::Named(NamedKey::Print),
        w::NamedKey::Save => Key::Named(NamedKey::Save),
        w::NamedKey::SpellCheck => Key::Named(NamedKey::SpellCheck),
        w::NamedKey::Key11 => Key::Named(NamedKey::Key11),
        w::NamedKey::Key12 => Key::Named(NamedKey::Key12),
        w::NamedKey::AudioBalanceLeft => Key::Named(NamedKey::AudioBalanceLeft),
        w::NamedKey::AudioBalanceRight => Key::Named(NamedKey::AudioBalanceRight),
        w::NamedKey::AudioBassBoostDown => Key::Named(NamedKey::AudioBassBoostDown),
        w::NamedKey::AudioBassBoostToggle => Key::Named(NamedKey::AudioBassBoostToggle),
        w::NamedKey::AudioBassBoostUp => Key::Named(NamedKey::AudioBassBoostUp),
        w::NamedKey::AudioFaderFront => Key::Named(NamedKey::AudioFaderFront),
        w::NamedKey::AudioFaderRear => Key::Named(NamedKey::AudioFaderRear),
        w::NamedKey::AudioSurroundModeNext => Key::Named(NamedKey::AudioSurroundModeNext),
        w::NamedKey::AudioTrebleDown => Key::Named(NamedKey::AudioTrebleDown),
        w::NamedKey::AudioTrebleUp => Key::Named(NamedKey::AudioTrebleUp),
        w::NamedKey::AudioVolumeDown => Key::Named(NamedKey::AudioVolumeDown),
        w::NamedKey::AudioVolumeUp => Key::Named(NamedKey::AudioVolumeUp),
        w::NamedKey::AudioVolumeMute => Key::Named(NamedKey::AudioVolumeMute),
        w::NamedKey::MicrophoneToggle => Key::Named(NamedKey::MicrophoneToggle),
        w::NamedKey::MicrophoneVolumeDown => Key::Named(NamedKey::MicrophoneVolumeDown),
        w::NamedKey::MicrophoneVolumeUp => Key::Named(NamedKey::MicrophoneVolumeUp),
        w::NamedKey::MicrophoneVolumeMute => Key::Named(NamedKey::MicrophoneVolumeMute),
        w::NamedKey::SpeechCorrectionList => Key::Named(NamedKey::SpeechCorrectionList),
        w::NamedKey::SpeechInputToggle => Key::Named(NamedKey::SpeechInputToggle),
        w::NamedKey::LaunchApplication1 => Key::Named(NamedKey::LaunchApplication1),
        w::NamedKey::LaunchApplication2 => Key::Named(NamedKey::LaunchApplication2),
        w::NamedKey::LaunchCalendar => Key::Named(NamedKey::LaunchCalendar),
        w::NamedKey::LaunchContacts => Key::Named(NamedKey::LaunchContacts),
        w::NamedKey::LaunchMail => Key::Named(NamedKey::LaunchMail),
        w::NamedKey::LaunchMediaPlayer => Key::Named(NamedKey::LaunchMediaPlayer),
        w::NamedKey::LaunchMusicPlayer => Key::Named(NamedKey::LaunchMusicPlayer),
        w::NamedKey::LaunchPhone => Key::Named(NamedKey::LaunchPhone),
        w::NamedKey::LaunchScreenSaver => Key::Named(NamedKey::LaunchScreenSaver),
        w::NamedKey::LaunchSpreadsheet => Key::Named(NamedKey::LaunchSpreadsheet),
        w::NamedKey::LaunchWebBrowser => Key::Named(NamedKey::LaunchWebBrowser),
        w::NamedKey::LaunchWebCam => Key::Named(NamedKey::LaunchWebCam),
        w::NamedKey::LaunchWordProcessor => Key::Named(NamedKey::LaunchWordProcessor),
        w::NamedKey::BrowserBack => Key::Named(NamedKey::BrowserBack),
        w::NamedKey::BrowserFavorites => Key::Named(NamedKey::BrowserFavorites),
        w::NamedKey::BrowserForward => Key::Named(NamedKey::BrowserForward),
        w::NamedKey::BrowserHome => Key::Named(NamedKey::BrowserHome),
        w::NamedKey::BrowserRefresh => Key::Named(NamedKey::BrowserRefresh),
        w::NamedKey::BrowserSearch => Key::Named(NamedKey::BrowserSearch),
        w::NamedKey::BrowserStop => Key::Named(NamedKey::BrowserStop),
        w::NamedKey::AppSwitch => Key::Named(NamedKey::AppSwitch),
        w::NamedKey::Call => Key::Named(NamedKey::Call),
        w::NamedKey::Camera => Key::Named(NamedKey::Camera),
        w::NamedKey::CameraFocus => Key::Named(NamedKey::CameraFocus),
        w::NamedKey::EndCall => Key::Named(NamedKey::EndCall),
        w::NamedKey::GoBack => Key::Named(NamedKey::GoBack),
        w::NamedKey::GoHome => Key::Named(NamedKey::GoHome),
        w::NamedKey::HeadsetHook => Key::Named(NamedKey::HeadsetHook),
        w::NamedKey::LastNumberRedial => Key::Named(NamedKey::LastNumberRedial),
        w::NamedKey::Notification => Key::Named(NamedKey::Notification),
        w::NamedKey::MannerMode => Key::Named(NamedKey::MannerMode),
        w::NamedKey::VoiceDial => Key::Named(NamedKey::VoiceDial),
        w::NamedKey::TV => Key::Named(NamedKey::TV),
        w::NamedKey::TV3DMode => Key::Named(NamedKey::TV3DMode),
        w::NamedKey::TVAntennaCable => Key::Named(NamedKey::TVAntennaCable),
        w::NamedKey::TVAudioDescription => Key::Named(NamedKey::TVAudioDescription),
        w::NamedKey::TVAudioDescriptionMixDown => Key::Named(NamedKey::TVAudioDescriptionMixDown),
        w::NamedKey::TVAudioDescriptionMixUp => Key::Named(NamedKey::TVAudioDescriptionMixUp),
        w::NamedKey::TVContentsMenu => Key::Named(NamedKey::TVContentsMenu),
        w::NamedKey::TVDataService => Key::Named(NamedKey::TVDataService),
        w::NamedKey::TVInput => Key::Named(NamedKey::TVInput),
        w::NamedKey::TVInputComponent1 => Key::Named(NamedKey::TVInputComponent1),
        w::NamedKey::TVInputComponent2 => Key::Named(NamedKey::TVInputComponent2),
        w::NamedKey::TVInputComposite1 => Key::Named(NamedKey::TVInputComposite1),
        w::NamedKey::TVInputComposite2 => Key::Named(NamedKey::TVInputComposite2),
        w::NamedKey::TVInputHDMI1 => Key::Named(NamedKey::TVInputHDMI1),
        w::NamedKey::TVInputHDMI2 => Key::Named(NamedKey::TVInputHDMI2),
        w::NamedKey::TVInputHDMI3 => Key::Named(NamedKey::TVInputHDMI3),
        w::NamedKey::TVInputHDMI4 => Key::Named(NamedKey::TVInputHDMI4),
        w::NamedKey::TVInputVGA1 => Key::Named(NamedKey::TVInputVGA1),
        w::NamedKey::TVMediaContext => Key::Named(NamedKey::TVMediaContext),
        w::NamedKey::TVNetwork => Key::Named(NamedKey::TVNetwork),
        w::NamedKey::TVNumberEntry => Key::Named(NamedKey::TVNumberEntry),
        w::NamedKey::TVPower => Key::Named(NamedKey::TVPower),
        w::NamedKey::TVRadioService => Key::Named(NamedKey::TVRadioService),
        w::NamedKey::TVSatellite => Key::Named(NamedKey::TVSatellite),
        w::NamedKey::TVSatelliteBS => Key::Named(NamedKey::TVSatelliteBS),
        w::NamedKey::TVSatelliteCS => Key::Named(NamedKey::TVSatelliteCS),
        w::NamedKey::TVSatelliteToggle => Key::Named(NamedKey::TVSatelliteToggle),
        w::NamedKey::TVTerrestrialAnalog => Key::Named(NamedKey::TVTerrestrialAnalog),
        w::NamedKey::TVTerrestrialDigital => Key::Named(NamedKey::TVTerrestrialDigital),
        w::NamedKey::TVTimer => Key::Named(NamedKey::TVTimer),
        w::NamedKey::AVRInput => Key::Named(NamedKey::AVRInput),
        w::NamedKey::AVRPower => Key::Named(NamedKey::AVRPower),
        w::NamedKey::ColorF0Red => Key::Named(NamedKey::ColorF0Red),
        w::NamedKey::ColorF1Green => Key::Named(NamedKey::ColorF1Green),
        w::NamedKey::ColorF2Yellow => Key::Named(NamedKey::ColorF2Yellow),
        w::NamedKey::ColorF3Blue => Key::Named(NamedKey::ColorF3Blue),
        w::NamedKey::ColorF4Grey => Key::Named(NamedKey::ColorF4Grey),
        w::NamedKey::ColorF5Brown => Key::Named(NamedKey::ColorF5Brown),
        w::NamedKey::ClosedCaptionToggle => Key::Named(NamedKey::ClosedCaptionToggle),
        w::NamedKey::Dimmer => Key::Named(NamedKey::Dimmer),
        w::NamedKey::DisplaySwap => Key::Named(NamedKey::DisplaySwap),
        w::NamedKey::DVR => Key::Named(NamedKey::DVR),
        w::NamedKey::Exit => Key::Named(NamedKey::Exit),
        w::NamedKey::FavoriteClear0 => Key::Named(NamedKey::FavoriteClear0),
        w::NamedKey::FavoriteClear1 => Key::Named(NamedKey::FavoriteClear1),
        w::NamedKey::FavoriteClear2 => Key::Named(NamedKey::FavoriteClear2),
        w::NamedKey::FavoriteClear3 => Key::Named(NamedKey::FavoriteClear3),
        w::NamedKey::FavoriteRecall0 => Key::Named(NamedKey::FavoriteRecall0),
        w::NamedKey::FavoriteRecall1 => Key::Named(NamedKey::FavoriteRecall1),
        w::NamedKey::FavoriteRecall2 => Key::Named(NamedKey::FavoriteRecall2),
        w::NamedKey::FavoriteRecall3 => Key::Named(NamedKey::FavoriteRecall3),
        w::NamedKey::FavoriteStore0 => Key::Named(NamedKey::FavoriteStore0),
        w::NamedKey::FavoriteStore1 => Key::Named(NamedKey::FavoriteStore1),
        w::NamedKey::FavoriteStore2 => Key::Named(NamedKey::FavoriteStore2),
        w::NamedKey::FavoriteStore3 => Key::Named(NamedKey::FavoriteStore3),
        w::NamedKey::Guide => Key::Named(NamedKey::Guide),
        w::NamedKey::GuideNextDay => Key::Named(NamedKey::GuideNextDay),
        w::NamedKey::GuidePreviousDay => Key::Named(NamedKey::GuidePreviousDay),
        w::NamedKey::Info => Key::Named(NamedKey::Info),
        w::NamedKey::InstantReplay => Key::Named(NamedKey::InstantReplay),
        w::NamedKey::Link => Key::Named(NamedKey::Link),
        w::NamedKey::ListProgram => Key::Named(NamedKey::ListProgram),
        w::NamedKey::LiveContent => Key::Named(NamedKey::LiveContent),
        w::NamedKey::Lock => Key::Named(NamedKey::Lock),
        w::NamedKey::MediaApps => Key::Named(NamedKey::MediaApps),
        w::NamedKey::MediaAudioTrack => Key::Named(NamedKey::MediaAudioTrack),
        w::NamedKey::MediaLast => Key::Named(NamedKey::MediaLast),
        w::NamedKey::MediaSkipBackward => Key::Named(NamedKey::MediaSkipBackward),
        w::NamedKey::MediaSkipForward => Key::Named(NamedKey::MediaSkipForward),
        w::NamedKey::MediaStepBackward => Key::Named(NamedKey::MediaStepBackward),
        w::NamedKey::MediaStepForward => Key::Named(NamedKey::MediaStepForward),
        w::NamedKey::MediaTopMenu => Key::Named(NamedKey::MediaTopMenu),
        w::NamedKey::NavigateIn => Key::Named(NamedKey::NavigateIn),
        w::NamedKey::NavigateNext => Key::Named(NamedKey::NavigateNext),
        w::NamedKey::NavigateOut => Key::Named(NamedKey::NavigateOut),
        w::NamedKey::NavigatePrevious => Key::Named(NamedKey::NavigatePrevious),
        w::NamedKey::NextFavoriteChannel => Key::Named(NamedKey::NextFavoriteChannel),
        w::NamedKey::NextUserProfile => Key::Named(NamedKey::NextUserProfile),
        w::NamedKey::OnDemand => Key::Named(NamedKey::OnDemand),
        w::NamedKey::Pairing => Key::Named(NamedKey::Pairing),
        w::NamedKey::PinPDown => Key::Named(NamedKey::PinPDown),
        w::NamedKey::PinPMove => Key::Named(NamedKey::PinPMove),
        w::NamedKey::PinPToggle => Key::Named(NamedKey::PinPToggle),
        w::NamedKey::PinPUp => Key::Named(NamedKey::PinPUp),
        w::NamedKey::PlaySpeedDown => Key::Named(NamedKey::PlaySpeedDown),
        w::NamedKey::PlaySpeedReset => Key::Named(NamedKey::PlaySpeedReset),
        w::NamedKey::PlaySpeedUp => Key::Named(NamedKey::PlaySpeedUp),
        w::NamedKey::RandomToggle => Key::Named(NamedKey::RandomToggle),
        w::NamedKey::RcLowBattery => Key::Named(NamedKey::RcLowBattery),
        w::NamedKey::RecordSpeedNext => Key::Named(NamedKey::RecordSpeedNext),
        w::NamedKey::RfBypass => Key::Named(NamedKey::RfBypass),
        w::NamedKey::ScanChannelsToggle => Key::Named(NamedKey::ScanChannelsToggle),
        w::NamedKey::ScreenModeNext => Key::Named(NamedKey::ScreenModeNext),
        w::NamedKey::Settings => Key::Named(NamedKey::Settings),
        w::NamedKey::SplitScreenToggle => Key::Named(NamedKey::SplitScreenToggle),
        w::NamedKey::STBInput => Key::Named(NamedKey::STBInput),
        w::NamedKey::STBPower => Key::Named(NamedKey::STBPower),
        w::NamedKey::Subtitle => Key::Named(NamedKey::Subtitle),
        w::NamedKey::Teletext => Key::Named(NamedKey::Teletext),
        w::NamedKey::VideoModeNext => Key::Named(NamedKey::VideoModeNext),
        w::NamedKey::Wink => Key::Named(NamedKey::Wink),
        w::NamedKey::ZoomToggle => Key::Named(NamedKey::ZoomToggle),
        w::NamedKey::F1 => Key::Named(NamedKey::F1),
        w::NamedKey::F2 => Key::Named(NamedKey::F2),
        w::NamedKey::F3 => Key::Named(NamedKey::F3),
        w::NamedKey::F4 => Key::Named(NamedKey::F4),
        w::NamedKey::F5 => Key::Named(NamedKey::F5),
        w::NamedKey::F6 => Key::Named(NamedKey::F6),
        w::NamedKey::F7 => Key::Named(NamedKey::F7),
        w::NamedKey::F8 => Key::Named(NamedKey::F8),
        w::NamedKey::F9 => Key::Named(NamedKey::F9),
        w::NamedKey::F10 => Key::Named(NamedKey::F10),
        w::NamedKey::F11 => Key::Named(NamedKey::F11),
        w::NamedKey::F12 => Key::Named(NamedKey::F12),
        w::NamedKey::F13 => Key::Named(NamedKey::F13),
        w::NamedKey::F14 => Key::Named(NamedKey::F14),
        w::NamedKey::F15 => Key::Named(NamedKey::F15),
        w::NamedKey::F16 => Key::Named(NamedKey::F16),
        w::NamedKey::F17 => Key::Named(NamedKey::F17),
        w::NamedKey::F18 => Key::Named(NamedKey::F18),
        w::NamedKey::F19 => Key::Named(NamedKey::F19),
        w::NamedKey::F20 => Key::Named(NamedKey::F20),
        w::NamedKey::F21 => Key::Named(NamedKey::F21),
        w::NamedKey::F22 => Key::Named(NamedKey::F22),
        w::NamedKey::F23 => Key::Named(NamedKey::F23),
        w::NamedKey::F24 => Key::Named(NamedKey::F24),
        w::NamedKey::F25 => Key::Named(NamedKey::F25),
        w::NamedKey::F26 => Key::Named(NamedKey::F26),
        w::NamedKey::F27 => Key::Named(NamedKey::F27),
        w::NamedKey::F28 => Key::Named(NamedKey::F28),
        w::NamedKey::F29 => Key::Named(NamedKey::F29),
        w::NamedKey::F30 => Key::Named(NamedKey::F30),
        w::NamedKey::F31 => Key::Named(NamedKey::F31),
        w::NamedKey::F32 => Key::Named(NamedKey::F32),
        w::NamedKey::F33 => Key::Named(NamedKey::F33),
        w::NamedKey::F34 => Key::Named(NamedKey::F34),
        w::NamedKey::F35 => Key::Named(NamedKey::F35),
        _ => Key::Named(NamedKey::Unidentified),
    }
}

/// The physical key: which button was pressed, regardless of layout.
pub(super) fn map_code(key: w::PhysicalKey) -> Code {
    let code = match key {
        w::PhysicalKey::Code(code) => code,
        w::PhysicalKey::Unidentified(_) => return Code::Unidentified,
    };
    match code {
        w::KeyCode::Backquote => Code::Backquote,
        w::KeyCode::Backslash => Code::Backslash,
        w::KeyCode::BracketLeft => Code::BracketLeft,
        w::KeyCode::BracketRight => Code::BracketRight,
        w::KeyCode::Comma => Code::Comma,
        w::KeyCode::Digit0 => Code::Digit0,
        w::KeyCode::Digit1 => Code::Digit1,
        w::KeyCode::Digit2 => Code::Digit2,
        w::KeyCode::Digit3 => Code::Digit3,
        w::KeyCode::Digit4 => Code::Digit4,
        w::KeyCode::Digit5 => Code::Digit5,
        w::KeyCode::Digit6 => Code::Digit6,
        w::KeyCode::Digit7 => Code::Digit7,
        w::KeyCode::Digit8 => Code::Digit8,
        w::KeyCode::Digit9 => Code::Digit9,
        w::KeyCode::Equal => Code::Equal,
        w::KeyCode::IntlBackslash => Code::IntlBackslash,
        w::KeyCode::IntlRo => Code::IntlRo,
        w::KeyCode::IntlYen => Code::IntlYen,
        w::KeyCode::KeyA => Code::KeyA,
        w::KeyCode::KeyB => Code::KeyB,
        w::KeyCode::KeyC => Code::KeyC,
        w::KeyCode::KeyD => Code::KeyD,
        w::KeyCode::KeyE => Code::KeyE,
        w::KeyCode::KeyF => Code::KeyF,
        w::KeyCode::KeyG => Code::KeyG,
        w::KeyCode::KeyH => Code::KeyH,
        w::KeyCode::KeyI => Code::KeyI,
        w::KeyCode::KeyJ => Code::KeyJ,
        w::KeyCode::KeyK => Code::KeyK,
        w::KeyCode::KeyL => Code::KeyL,
        w::KeyCode::KeyM => Code::KeyM,
        w::KeyCode::KeyN => Code::KeyN,
        w::KeyCode::KeyO => Code::KeyO,
        w::KeyCode::KeyP => Code::KeyP,
        w::KeyCode::KeyQ => Code::KeyQ,
        w::KeyCode::KeyR => Code::KeyR,
        w::KeyCode::KeyS => Code::KeyS,
        w::KeyCode::KeyT => Code::KeyT,
        w::KeyCode::KeyU => Code::KeyU,
        w::KeyCode::KeyV => Code::KeyV,
        w::KeyCode::KeyW => Code::KeyW,
        w::KeyCode::KeyX => Code::KeyX,
        w::KeyCode::KeyY => Code::KeyY,
        w::KeyCode::KeyZ => Code::KeyZ,
        w::KeyCode::Minus => Code::Minus,
        w::KeyCode::Period => Code::Period,
        w::KeyCode::Quote => Code::Quote,
        w::KeyCode::Semicolon => Code::Semicolon,
        w::KeyCode::Slash => Code::Slash,
        w::KeyCode::AltLeft => Code::AltLeft,
        w::KeyCode::AltRight => Code::AltRight,
        w::KeyCode::Backspace => Code::Backspace,
        w::KeyCode::CapsLock => Code::CapsLock,
        w::KeyCode::ContextMenu => Code::ContextMenu,
        w::KeyCode::ControlLeft => Code::ControlLeft,
        w::KeyCode::ControlRight => Code::ControlRight,
        w::KeyCode::Enter => Code::Enter,
        w::KeyCode::SuperLeft => Code::MetaLeft,
        w::KeyCode::SuperRight => Code::MetaRight,
        w::KeyCode::ShiftLeft => Code::ShiftLeft,
        w::KeyCode::ShiftRight => Code::ShiftRight,
        w::KeyCode::Space => Code::Space,
        w::KeyCode::Tab => Code::Tab,
        w::KeyCode::Convert => Code::Convert,
        w::KeyCode::KanaMode => Code::KanaMode,
        w::KeyCode::Lang1 => Code::Lang1,
        w::KeyCode::Lang2 => Code::Lang2,
        w::KeyCode::Lang3 => Code::Lang3,
        w::KeyCode::Lang4 => Code::Lang4,
        w::KeyCode::Lang5 => Code::Lang5,
        w::KeyCode::NonConvert => Code::NonConvert,
        w::KeyCode::Delete => Code::Delete,
        w::KeyCode::End => Code::End,
        w::KeyCode::Help => Code::Help,
        w::KeyCode::Home => Code::Home,
        w::KeyCode::Insert => Code::Insert,
        w::KeyCode::PageDown => Code::PageDown,
        w::KeyCode::PageUp => Code::PageUp,
        w::KeyCode::ArrowDown => Code::ArrowDown,
        w::KeyCode::ArrowLeft => Code::ArrowLeft,
        w::KeyCode::ArrowRight => Code::ArrowRight,
        w::KeyCode::ArrowUp => Code::ArrowUp,
        w::KeyCode::NumLock => Code::NumLock,
        w::KeyCode::Numpad0 => Code::Numpad0,
        w::KeyCode::Numpad1 => Code::Numpad1,
        w::KeyCode::Numpad2 => Code::Numpad2,
        w::KeyCode::Numpad3 => Code::Numpad3,
        w::KeyCode::Numpad4 => Code::Numpad4,
        w::KeyCode::Numpad5 => Code::Numpad5,
        w::KeyCode::Numpad6 => Code::Numpad6,
        w::KeyCode::Numpad7 => Code::Numpad7,
        w::KeyCode::Numpad8 => Code::Numpad8,
        w::KeyCode::Numpad9 => Code::Numpad9,
        w::KeyCode::NumpadAdd => Code::NumpadAdd,
        w::KeyCode::NumpadBackspace => Code::NumpadBackspace,
        w::KeyCode::NumpadClear => Code::NumpadClear,
        w::KeyCode::NumpadClearEntry => Code::NumpadClearEntry,
        w::KeyCode::NumpadComma => Code::NumpadComma,
        w::KeyCode::NumpadDecimal => Code::NumpadDecimal,
        w::KeyCode::NumpadDivide => Code::NumpadDivide,
        w::KeyCode::NumpadEnter => Code::NumpadEnter,
        w::KeyCode::NumpadEqual => Code::NumpadEqual,
        w::KeyCode::NumpadHash => Code::NumpadHash,
        w::KeyCode::NumpadMemoryAdd => Code::NumpadMemoryAdd,
        w::KeyCode::NumpadMemoryClear => Code::NumpadMemoryClear,
        w::KeyCode::NumpadMemoryRecall => Code::NumpadMemoryRecall,
        w::KeyCode::NumpadMemoryStore => Code::NumpadMemoryStore,
        w::KeyCode::NumpadMemorySubtract => Code::NumpadMemorySubtract,
        w::KeyCode::NumpadMultiply => Code::NumpadMultiply,
        w::KeyCode::NumpadParenLeft => Code::NumpadParenLeft,
        w::KeyCode::NumpadParenRight => Code::NumpadParenRight,
        w::KeyCode::NumpadStar => Code::NumpadStar,
        w::KeyCode::NumpadSubtract => Code::NumpadSubtract,
        w::KeyCode::Escape => Code::Escape,
        w::KeyCode::Fn => Code::Fn,
        w::KeyCode::FnLock => Code::FnLock,
        w::KeyCode::PrintScreen => Code::PrintScreen,
        w::KeyCode::ScrollLock => Code::ScrollLock,
        w::KeyCode::Pause => Code::Pause,
        w::KeyCode::BrowserBack => Code::BrowserBack,
        w::KeyCode::BrowserFavorites => Code::BrowserFavorites,
        w::KeyCode::BrowserForward => Code::BrowserForward,
        w::KeyCode::BrowserHome => Code::BrowserHome,
        w::KeyCode::BrowserRefresh => Code::BrowserRefresh,
        w::KeyCode::BrowserSearch => Code::BrowserSearch,
        w::KeyCode::BrowserStop => Code::BrowserStop,
        w::KeyCode::Eject => Code::Eject,
        w::KeyCode::LaunchApp1 => Code::LaunchApp1,
        w::KeyCode::LaunchApp2 => Code::LaunchApp2,
        w::KeyCode::LaunchMail => Code::LaunchMail,
        w::KeyCode::MediaPlayPause => Code::MediaPlayPause,
        w::KeyCode::MediaSelect => Code::MediaSelect,
        w::KeyCode::MediaStop => Code::MediaStop,
        w::KeyCode::MediaTrackNext => Code::MediaTrackNext,
        w::KeyCode::MediaTrackPrevious => Code::MediaTrackPrevious,
        w::KeyCode::Power => Code::Power,
        w::KeyCode::Sleep => Code::Sleep,
        w::KeyCode::AudioVolumeDown => Code::AudioVolumeDown,
        w::KeyCode::AudioVolumeMute => Code::AudioVolumeMute,
        w::KeyCode::AudioVolumeUp => Code::AudioVolumeUp,
        w::KeyCode::WakeUp => Code::WakeUp,
        w::KeyCode::Meta => Code::Super,
        w::KeyCode::Hyper => Code::Hyper,
        w::KeyCode::Turbo => Code::Turbo,
        w::KeyCode::Abort => Code::Abort,
        w::KeyCode::Resume => Code::Resume,
        w::KeyCode::Suspend => Code::Suspend,
        w::KeyCode::Again => Code::Again,
        w::KeyCode::Copy => Code::Copy,
        w::KeyCode::Cut => Code::Cut,
        w::KeyCode::Find => Code::Find,
        w::KeyCode::Open => Code::Open,
        w::KeyCode::Paste => Code::Paste,
        w::KeyCode::Props => Code::Props,
        w::KeyCode::Select => Code::Select,
        w::KeyCode::Undo => Code::Undo,
        w::KeyCode::Hiragana => Code::Hiragana,
        w::KeyCode::Katakana => Code::Katakana,
        w::KeyCode::F1 => Code::F1,
        w::KeyCode::F2 => Code::F2,
        w::KeyCode::F3 => Code::F3,
        w::KeyCode::F4 => Code::F4,
        w::KeyCode::F5 => Code::F5,
        w::KeyCode::F6 => Code::F6,
        w::KeyCode::F7 => Code::F7,
        w::KeyCode::F8 => Code::F8,
        w::KeyCode::F9 => Code::F9,
        w::KeyCode::F10 => Code::F10,
        w::KeyCode::F11 => Code::F11,
        w::KeyCode::F12 => Code::F12,
        w::KeyCode::F13 => Code::F13,
        w::KeyCode::F14 => Code::F14,
        w::KeyCode::F15 => Code::F15,
        w::KeyCode::F16 => Code::F16,
        w::KeyCode::F17 => Code::F17,
        w::KeyCode::F18 => Code::F18,
        w::KeyCode::F19 => Code::F19,
        w::KeyCode::F20 => Code::F20,
        w::KeyCode::F21 => Code::F21,
        w::KeyCode::F22 => Code::F22,
        w::KeyCode::F23 => Code::F23,
        w::KeyCode::F24 => Code::F24,
        w::KeyCode::F25 => Code::F25,
        w::KeyCode::F26 => Code::F26,
        w::KeyCode::F27 => Code::F27,
        w::KeyCode::F28 => Code::F28,
        w::KeyCode::F29 => Code::F29,
        w::KeyCode::F30 => Code::F30,
        w::KeyCode::F31 => Code::F31,
        w::KeyCode::F32 => Code::F32,
        w::KeyCode::F33 => Code::F33,
        w::KeyCode::F34 => Code::F34,
        w::KeyCode::F35 => Code::F35,
        _ => Code::Unidentified,
    }
}

/// Which of several same-named keys was pressed.
pub(super) fn map_location(location: w::KeyLocation) -> Location {
    match location {
        w::KeyLocation::Standard => Location::Standard,
        w::KeyLocation::Left => Location::Left,
        w::KeyLocation::Right => Location::Right,
        w::KeyLocation::Numpad => Location::Numpad,
    }
}

/// The modifiers held at the moment of the event.
///
/// winit tracks only these four; the spec's lock and symbol flags
/// (`CAPS_LOCK`, `NUM_LOCK`, `FN`, ...) have no winit counterpart and stay
/// clear. `SUPER` is the spec's `META` — see the module docs.
pub(super) fn map_modifiers(state: w::ModifiersState) -> Modifiers {
    let mut out = Modifiers::empty();
    out.set(Modifiers::SHIFT, state.shift_key());
    out.set(Modifiers::CONTROL, state.control_key());
    out.set(Modifiers::ALT, state.alt_key());
    out.set(Modifiers::META, state.super_key());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // The identity arms are generated from the two enums' variant lists, so
    // they are only worth a spot check. What is worth pinning is every place
    // the two vocabularies disagree — those are hand-written, and a winit
    // upgrade that renamed one of them would otherwise fall silently into the
    // catch-all and turn a real key into `Unidentified`.

    #[test]
    fn a_shared_name_maps_to_itself() {
        assert_eq!(map_code(w::PhysicalKey::Code(w::KeyCode::KeyA)), Code::KeyA);
        assert_eq!(
            map_key(w::Key::Named(w::NamedKey::Enter)),
            Key::Named(NamedKey::Enter)
        );
    }

    #[test]
    fn the_space_bar_arrives_as_a_character_not_a_named_key() {
        // The spec has no `Space` named key: a widget must handle the space bar
        // in the same arm as every other text-producing key.
        assert_eq!(
            map_key(w::Key::Named(w::NamedKey::Space)),
            Key::Character(" ".to_string())
        );
    }

    #[test]
    fn winits_super_is_the_specs_meta_and_vice_versa() {
        assert_eq!(
            map_code(w::PhysicalKey::Code(w::KeyCode::SuperLeft)),
            Code::MetaLeft
        );
        assert_eq!(
            map_code(w::PhysicalKey::Code(w::KeyCode::SuperRight)),
            Code::MetaRight
        );
        assert_eq!(map_code(w::PhysicalKey::Code(w::KeyCode::Meta)), Code::Super);
        // And the modifier flag follows the same swap.
        assert_eq!(map_modifiers(w::ModifiersState::SUPER), Modifiers::META);
    }

    #[test]
    fn keys_the_platform_could_not_name_degrade_rather_than_panic() {
        assert_eq!(
            map_code(w::PhysicalKey::Unidentified(w::NativeKeyCode::Windows(
                0xffff
            ))),
            Code::Unidentified
        );
        assert_eq!(
            map_key(w::Key::Unidentified(w::NativeKey::Windows(0xffff))),
            Key::Named(NamedKey::Unidentified)
        );
        // winit reports a dead key's text separately; the spec puts it in the
        // composition events, so only the marker survives the mapping.
        assert_eq!(map_key(w::Key::Dead(Some('^'))), Key::Named(NamedKey::Dead));
    }

    #[test]
    fn every_modifier_winit_tracks_survives() {
        let all = w::ModifiersState::SHIFT
            | w::ModifiersState::CONTROL
            | w::ModifiersState::ALT
            | w::ModifiersState::SUPER;
        let mapped = map_modifiers(all);
        assert!(mapped.shift() && mapped.ctrl() && mapped.alt() && mapped.meta());
        assert!(map_modifiers(w::ModifiersState::empty()).is_empty());
    }
}
