// Wreckfest 2 "Pino" telemetry packet parser.
//
// Schema source: TelemetryDataFormatPino.h shipped inside the game install
// dir. All structs use `#pragma pack(1)`, mirrored here as `#[repr(C, packed)]`.
//
// The wire format is little-endian. We parse by copying the bytes out as the
// matching Rust struct via `ptr::read_unaligned`, which sidesteps alignment
// concerns at the cost of a single ~1KB copy per packet.

use std::ptr;

/// Default UDP port the game's telemetry config uses out of the box.
pub const DEFAULT_PORT: u16 = 23123;

/// Magic value at offset 0 of every Pino packet. Verbatim from the C header
/// (`const U32 signature = 1869769584`). Note: "Pino" is just the protocol's
/// internal codename — the actual little-endian bytes are `70 6B 72 6F`, not
/// the ASCII characters of the name.
pub const SIGNATURE: u32 = 1869769584;

pub const PARTICIPANTS_MAX: usize = 36;
pub const TRACK_ID_LEN: usize = 64;
pub const TRACK_NAME_LEN: usize = 96;
pub const CAR_ID_LEN: usize = 64;
pub const CAR_NAME_LEN: usize = 96;
pub const PLAYER_NAME_LEN: usize = 24;
pub const DAMAGE_PARTS_MAX: usize = 56;
pub const DAMAGE_BITS_PER_PART: usize = 3;
pub const DAMAGE_BYTES_PER_PARTICIPANT: usize =
    (DAMAGE_PARTS_MAX * DAMAGE_BITS_PER_PART).div_ceil(8); // 21

//
// ENUMS
//

macro_rules! u8_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($variant:ident = $val:expr),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[repr(u8)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis enum $name {
            $($variant = $val),*
        }

        impl $name {
            pub fn from_u8(b: u8) -> Option<Self> {
                match b {
                    $($val => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

u8_enum! {
    pub enum PacketType {
        Main = 0,
        ParticipantsLeaderboard = 1,
        ParticipantsTiming = 2,
        ParticipantsTimingSectors = 3,
        ParticipantsMotion = 4,
        ParticipantsInfo = 5,
        ParticipantsDamage = 6,
    }
}

u8_enum! {
    pub enum GameMode {
        Banger = 0,
        Deathmatch = 1,
        LastManStanding = 2,
        Other = 255,
    }
}

u8_enum! {
    pub enum DamageMode {
        Wrecker = 0,
        Normal = 1,
        Realistic = 2,
        Other = 255,
    }
}

u8_enum! {
    pub enum SessionStatus {
        None = 0,
        PreRace = 1,
        Countdown = 2,
        Racing = 3,
        Abandoned = 4,
        PostRace = 5,
    }
}

u8_enum! {
    pub enum SurfaceType {
        Default = 0,
        NoContact = 1,
        Tarmac = 2,
        Concrete = 3,
        Gravel = 4,
        Dirt = 5,
        Mud = 6,
        RumbleLowFreq = 7,
        RumbleHighFreq = 8,
        Water = 9,
        Metal = 10,
        Wood = 11,
        Sand = 12,
        Rocks = 13,
        Foliage = 14,
        Slowdown = 15,
        Snow = 16,
    }
}

u8_enum! {
    pub enum AssistGearbox {
        Auto = 0,
        Manual = 1,
        ManualWithClutch = 2,
    }
}

u8_enum! {
    pub enum AssistLevel {
        Off = 0,
        Half = 1,
        Full = 2,
    }
}

u8_enum! {
    pub enum DrivelineType {
        Fwd = 0,
        Rwd = 1,
        Awd = 2,
    }
}

u8_enum! {
    pub enum Visibility {
        Full = 0,
        Limited = 1,
    }
}

u8_enum! {
    pub enum ParticipantStatus {
        Invalid = 0,
        Unused = 1,
        Racing = 2,
        FinishSuccess = 3,
        FinishEliminated = 4,
        DnfDq = 5,
        DnfRetired = 6,
        DnfTimeout = 7,
        DnfWrecked = 8,
        Pose = 9,
    }
}

u8_enum! {
    pub enum TrackStatus {
        Normal = 0,
        OutOfSectors = 1,
        InWrongSectors = 2,
        WrongDirection = 3,
    }
}

u8_enum! {
    pub enum DamageState {
        Ok = 0,
        Damaged1 = 1,
        Damaged2 = 2,
        Damaged3 = 3,
        Terminal = 4,
    }
}

/// Damage part indices (0..=36). Used to interpret unpacked
/// `Participant::Damage::states()` array entries.
pub mod damage_part {
    pub const ENGINE: usize = 0;
    pub const GEARBOX: usize = 1;
    pub const BRAKE_FL: usize = 2;
    pub const BRAKE_FR: usize = 3;
    pub const BRAKE_RL: usize = 4;
    pub const BRAKE_RR: usize = 5;
    pub const SUSPENSION_FL: usize = 6;
    pub const SUSPENSION_FR: usize = 7;
    pub const SUSPENSION_RL: usize = 8;
    pub const SUSPENSION_RR: usize = 9;
    pub const TIRE_FL: usize = 10;
    pub const TIRE_FR: usize = 11;
    pub const TIRE_RL: usize = 12;
    pub const TIRE_RR: usize = 13;
    pub const HEAD_GASKET: usize = 14;
    pub const RADIATOR: usize = 15;
    pub const PISTONS: usize = 16;
    pub const TIRE_HUB_FL: usize = 17;
    pub const TIRE_HUB_FR: usize = 18;
    pub const TIRE_HUB_RL: usize = 19;
    pub const TIRE_HUB_RR: usize = 20;
    pub const OIL_PAN: usize = 21;
    pub const COOLANT: usize = 22;
    pub const OIL: usize = 23;
    pub const END_BEARINGS: usize = 24;
    pub const HALF_SHAFT_FL: usize = 25;
    pub const HALF_SHAFT_FR: usize = 26;
    pub const HALF_SHAFT_RL: usize = 27;
    pub const HALF_SHAFT_RR: usize = 28;
    pub const RADIATOR_LEAK: usize = 29;
    pub const ARMOR_FL: usize = 30;
    pub const ARMOR_FR: usize = 31;
    pub const ARMOR_RL: usize = 32;
    pub const ARMOR_RR: usize = 33;
    pub const ARMOR_SL: usize = 34;
    pub const ARMOR_SR: usize = 35;
    pub const MISFIRE: usize = 36;
    pub const COUNT: usize = 37;
}

//
// BITFLAG NEWTYPES
//

macro_rules! bitflags {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident($repr:ty) {
            $(const $flag:ident = $val:expr;)*
        }
    ) => {
        $(#[$meta])*
        #[repr(transparent)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        $vis struct $name(pub $repr);

        impl $name {
            $(pub const $flag: Self = Self($val);)*

            pub const fn bits(self) -> $repr {
                self.0
            }

            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }
        }
    };
}

bitflags! {
    pub struct GameStatusFlags(u8) {
        const PAUSED = 1 << 0;
        const REPLAY = 1 << 1;
        const SPECTATE = 1 << 2;
        const MULTIPLAYER_CLIENT = 1 << 3;
        const MULTIPLAYER_SERVER = 1 << 4;
        const IN_RACE = 1 << 5;
    }
}

bitflags! {
    pub struct MarshalFlags(u16) {
        const GREEN = 1 << 0;
        const LAST_LAP = 1 << 1;
        const FINISH = 1 << 2;
        const DQ = 1 << 3;
        const MEATBALL = 1 << 4;
        const WARNING = 1 << 5;
        const BLUE = 1 << 6;
        const WHITE = 1 << 7;
        const COUNTDOWN1 = 1 << 14;
        const COUNTDOWN2 = 1 << 15;
    }
}

bitflags! {
    pub struct PlayerStatusFlags(u16) {
        const IN_RACE = 1 << 0;
        const CAR_DRIVABLE = 1 << 1;
        const PHYSICS_RUNNING = 1 << 2;
        const CONTROL_PLAYER = 1 << 3;
        const CONTROL_AI = 1 << 4;
    }
}

bitflags! {
    pub struct AssistFlags(u8) {
        const ABS_ACTIVE = 1 << 0;
        const TCS_ACTIVE = 1 << 1;
        const ESC_ACTIVE = 1 << 2;
    }
}

bitflags! {
    pub struct EngineFlags(u8) {
        const RUNNING = 1 << 0;
        const STARTING = 1 << 1;
        const MISFIRING = 1 << 2;
        const DANGER_TO_MANIFOLD = 1 << 7;
    }
}

bitflags! {
    pub struct InputFlags(u8) {
        const FFB_ENABLED = 1 << 0;
    }
}

//
// CHILD STRUCTS
//
// Keep field names mirroring the C header so cross-referencing stays trivial.
// Numeric flag/enum fields are kept as the raw integer type (`u8`/`u16`) for
// `#[repr(C, packed)]` simplicity; the wrapper accessor methods below convert
// them to the typed enum/bitflag variants.

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Header {
    pub signature: u32,
    pub packet_type: u8,
    pub status_flags: u8,
    pub session_time: i32,
    pub race_time: i32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Session {
    pub track_id: [u8; TRACK_ID_LEN],
    pub track_name: [u8; TRACK_NAME_LEN],
    pub track_length: f32,
    pub laps: i16,
    pub event_length: i16,
    pub grid_size: u8,
    pub grid_size_remaining: u8,
    pub sector_count: u8,
    pub sector_fract1: f32,
    pub sector_fract2: f32,
    pub game_mode: u8,
    pub damage_mode: u8,
    pub status: u8,
    pub _reserved: [u8; 26],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Orientation {
    pub position_x: f32,
    pub position_y: f32,
    pub position_z: f32,
    pub orientation_quat_x: f32,
    pub orientation_quat_y: f32,
    pub orientation_quat_z: f32,
    pub orientation_quat_w: f32,
    pub extents_x: u16,
    pub extents_y: u16,
    pub extents_z: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Velocity {
    pub velocity_local_x: f32,
    pub velocity_local_y: f32,
    pub velocity_local_z: f32,
    pub angular_velocity_x: f32,
    pub angular_velocity_y: f32,
    pub angular_velocity_z: f32,
    pub acceleration_local_x: f32,
    pub acceleration_local_y: f32,
    pub acceleration_local_z: f32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct VelocityEssential {
    pub velocity_magnitude: f32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Assists {
    pub flags: u8,
    pub assist_gearbox: u8,
    pub level_abs: u8,
    pub level_tcs: u8,
    pub level_esc: u8,
    pub _reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Chassis {
    pub track_width_front: f32,
    pub track_width_rear: f32,
    pub wheel_base: f32,
    pub steering_wheel_lock_to_lock: i32,
    pub steering_lock: f32,
    pub corner_weight_fl: f32,
    pub corner_weight_fr: f32,
    pub corner_weight_rl: f32,
    pub corner_weight_rr: f32,
    pub _reserved: [u8; 16],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Driveline {
    pub driveline_type: u8,
    /// 0 = R, 1 = N, 2 = 1st, ...
    pub gear: u8,
    pub gear_max: u8,
    pub speed: f32, // m/s
    pub _reserved: [u8; 17],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Engine {
    pub flags: u8,
    pub rpm: i32,
    pub rpm_max: i32,     // limiter
    pub rpm_redline: i32, // upshift
    pub rpm_idle: i32,
    pub torque: f32,            // Nm
    pub power: f32,             // W
    pub temp_block: f32,        // K
    pub temp_water: f32,        // K
    pub pressure_manifold: f32, // kPa
    pub pressure_oil: f32,      // kPa
    pub _reserved: [u8; 15],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Input {
    pub throttle: f32,
    pub brake: f32,
    pub clutch: f32,
    pub handbrake: f32,
    pub steering: f32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Tire {
    pub rps: f32,
    pub camber: f32,
    pub slip_ratio: f32,
    pub slip_angle: f32,
    pub radius_unloaded: f32,
    pub load_vertical: f32,
    pub force_lat: f32,
    pub force_long: f32,
    pub temperature_inner: f32,
    pub temperature_tread: f32,
    pub suspension_velocity: f32,
    pub suspension_displacement: f32,
    pub suspension_disp_norm: f32,
    pub position_vertical: f32,
    pub surface_type: u8,
    pub _reserved: [u8; 15],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct CarFull {
    pub assists: Assists,
    pub chassis: Chassis,
    pub driveline: Driveline,
    pub engine: Engine,
    pub input: Input,
    pub orientation: Orientation,
    pub velocity: Velocity,
    /// Indexed by `TIRE_LOC_*` constants below: FL=0, FR=1, RL=2, RR=3.
    pub tires: [Tire; 4],
    pub _reserved: [u8; 14],
}

pub const TIRE_LOC_FL: usize = 0;
pub const TIRE_LOC_FR: usize = 1;
pub const TIRE_LOC_RL: usize = 2;
pub const TIRE_LOC_RR: usize = 3;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct InputExtended {
    pub flags: u8,
    pub ffb_force: f32,
    pub _reserved: [u8; 15],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Leaderboard {
    pub status: u8,
    pub track_status: u8,
    pub lap_current: u16,
    pub position: u8,
    pub health: u8,
    pub wrecks: u16,
    pub frags: u16,
    pub assists: u16,
    pub score: i32,
    pub points: i32,
    pub delta_leader: i32, // ms
    pub lap_timing: u16,
    pub _reserved: [u8; 6],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Timing {
    pub lap_time_current: u32,         // ms (with penalty)
    pub lap_time_penalty_current: u32, // ms
    pub lap_time_last: u32,            // ms
    pub lap_time_best: u32,            // ms
    pub lap_best: u8,
    pub delta_ahead: i32,  // ms, -1 = N/A
    pub delta_behind: i32, // ms, -1 = N/A
    pub lap_progress: f32, // 0..=1
    pub _reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TimingSectors {
    pub sector_time_current_lap_1: u32,
    pub sector_time_current_lap_2: u32,
    pub sector_time_last_lap_1: u32,
    pub sector_time_last_lap_2: u32,
    pub sector_time_best_lap_1: u32,
    pub sector_time_best_lap_2: u32,
    pub sector_time_best_1: u32,
    pub sector_time_best_2: u32,
    pub sector_time_best_3: u32,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantMotion {
    pub orientation: Orientation,
    pub velocity: VelocityEssential,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Damage {
    /// Bit-packed: 3 bits per part, `DAMAGE_PARTS_MAX` parts.
    /// Use [`Damage::states`] to unpack into a `[u8; damage_part::COUNT]`.
    pub damage_states: [u8; DAMAGE_BYTES_PER_PARTICIPANT],
}

impl Damage {
    /// Decompress the bit-packed states into one byte per part, indexed by
    /// the constants in [`damage_part`].
    pub fn states(&self) -> [u8; damage_part::COUNT] {
        let mut out = [0u8; damage_part::COUNT];
        let comp = self.damage_states;
        for (i, slot) in out.iter_mut().enumerate() {
            let bit_pos = i * DAMAGE_BITS_PER_PART;
            let byte_index = bit_pos / 8;
            let bit_offset = bit_pos % 8;
            let mut state = (comp[byte_index] >> bit_offset) & 0b111;
            if bit_offset > 5 && byte_index + 1 < comp.len() {
                state |= (comp[byte_index + 1] << (8 - bit_offset)) & 0b111;
            }
            *slot = state;
        }
        out
    }
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Info {
    pub car_id: [u8; CAR_ID_LEN],
    pub car_name: [u8; CAR_NAME_LEN],
    pub player_name: [u8; PLAYER_NAME_LEN],
    pub participant_index: u8,
    pub last_normal_track_status_time: i32, // ms
    pub last_collision_time: i32,           // ms
    pub last_reset_time: i32,               // ms
    pub _reserved: [u8; 16],
}

//
// PACKETS
//

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Main {
    pub header: Header,
    pub marshal_flags_player: u16,
    pub leaderboard: Leaderboard,
    pub timing: Timing,
    pub timing_sectors: TimingSectors,
    pub info: Info,
    pub damage: Damage,
    pub car: CarFull,
    pub session: Session,
    pub player_status_flags: u16,
    pub input_extended: InputExtended,
    pub _reserved: [u8; 106],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsLeaderboard {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [Leaderboard; PARTICIPANTS_MAX],
    pub _reserved: [u8; 64],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsTiming {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [Timing; PARTICIPANTS_MAX],
    pub _reserved: [u8; 64],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsTimingSectors {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [TimingSectors; PARTICIPANTS_MAX],
    pub _reserved: [u8; 64],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsMotion {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [ParticipantMotion; PARTICIPANTS_MAX],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsInfo {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [Info; PARTICIPANTS_MAX],
    pub _reserved: [u8; 512],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct ParticipantsDamage {
    pub header: Header,
    pub participant_visibility: u8,
    pub participants: [Damage; PARTICIPANTS_MAX],
    pub _reserved: [u8; 256],
}

//
// TYPED ACCESSORS
//
// Wrapper methods convert raw u8/u16 fields into the typed enum/bitflag
// representations. These read the underlying integer with `addr_of!` to
// avoid taking references into a packed struct.

macro_rules! packed_get {
    ($self:ident.$field:ident, $ty:ty) => {{
        let p = ptr::addr_of!($self.$field);
        unsafe { ptr::read_unaligned(p) as $ty }
    }};
}

impl Header {
    pub fn packet_type(&self) -> Option<PacketType> {
        PacketType::from_u8(self.packet_type)
    }
    pub fn status_flags(&self) -> GameStatusFlags {
        GameStatusFlags(self.status_flags)
    }
}

impl Session {
    pub fn game_mode(&self) -> Option<GameMode> {
        GameMode::from_u8(self.game_mode)
    }
    pub fn damage_mode(&self) -> Option<DamageMode> {
        DamageMode::from_u8(self.damage_mode)
    }
    pub fn status(&self) -> Option<SessionStatus> {
        SessionStatus::from_u8(self.status)
    }
}

impl Assists {
    pub fn flags(&self) -> AssistFlags {
        AssistFlags(self.flags)
    }
    pub fn gearbox(&self) -> Option<AssistGearbox> {
        AssistGearbox::from_u8(self.assist_gearbox)
    }
    pub fn abs(&self) -> Option<AssistLevel> {
        AssistLevel::from_u8(self.level_abs)
    }
    pub fn tcs(&self) -> Option<AssistLevel> {
        AssistLevel::from_u8(self.level_tcs)
    }
    pub fn esc(&self) -> Option<AssistLevel> {
        AssistLevel::from_u8(self.level_esc)
    }
}

impl Driveline {
    pub fn driveline_type(&self) -> Option<DrivelineType> {
        DrivelineType::from_u8(self.driveline_type)
    }
}

impl Engine {
    pub fn flags(&self) -> EngineFlags {
        EngineFlags(self.flags)
    }
}

impl Tire {
    pub fn surface_type(&self) -> Option<SurfaceType> {
        SurfaceType::from_u8(self.surface_type)
    }
}

impl InputExtended {
    pub fn flags(&self) -> InputFlags {
        InputFlags(self.flags)
    }
}

impl Leaderboard {
    pub fn status(&self) -> Option<ParticipantStatus> {
        ParticipantStatus::from_u8(self.status)
    }
    pub fn track_status(&self) -> Option<TrackStatus> {
        TrackStatus::from_u8(self.track_status)
    }
}

impl Main {
    pub fn marshal_flags(&self) -> MarshalFlags {
        MarshalFlags(packed_get!(self.marshal_flags_player, u16))
    }
    pub fn player_status(&self) -> PlayerStatusFlags {
        PlayerStatusFlags(packed_get!(self.player_status_flags, u16))
    }
}

//
// PARSING
//

/// Top-level parsed packet. Each variant is an owned, byte-for-byte copy
/// of the wire bytes — references into `buf` would run afoul of packed
/// alignment rules. Variant size differs significantly (Damage ~1KB,
/// Info ~8KB); fine since at most one Packet exists at a time on the stack.
#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)]
pub enum Packet {
    Main(Main),
    ParticipantsLeaderboard(ParticipantsLeaderboard),
    ParticipantsTiming(ParticipantsTiming),
    ParticipantsTimingSectors(ParticipantsTimingSectors),
    ParticipantsMotion(ParticipantsMotion),
    ParticipantsInfo(ParticipantsInfo),
    ParticipantsDamage(ParticipantsDamage),
}

impl Packet {
    pub fn header(&self) -> Header {
        match self {
            Packet::Main(p) => p.header,
            Packet::ParticipantsLeaderboard(p) => p.header,
            Packet::ParticipantsTiming(p) => p.header,
            Packet::ParticipantsTimingSectors(p) => p.header,
            Packet::ParticipantsMotion(p) => p.header,
            Packet::ParticipantsInfo(p) => p.header,
            Packet::ParticipantsDamage(p) => p.header,
        }
    }
}

/// Parse any Pino packet. Returns `None` if the buffer is too short for the
/// header, the signature doesn't match, the packet type is unknown, or the
/// buffer is shorter than the packet's declared layout.
pub fn parse(buf: &[u8]) -> Option<Packet> {
    let header: Header = read_unaligned(buf)?;
    if header.signature != SIGNATURE {
        return None;
    }
    let pt = header.packet_type()?;
    match pt {
        PacketType::Main => read_unaligned::<Main>(buf).map(Packet::Main),
        PacketType::ParticipantsLeaderboard => {
            read_unaligned::<ParticipantsLeaderboard>(buf).map(Packet::ParticipantsLeaderboard)
        }
        PacketType::ParticipantsTiming => {
            read_unaligned::<ParticipantsTiming>(buf).map(Packet::ParticipantsTiming)
        }
        PacketType::ParticipantsTimingSectors => {
            read_unaligned::<ParticipantsTimingSectors>(buf).map(Packet::ParticipantsTimingSectors)
        }
        PacketType::ParticipantsMotion => {
            read_unaligned::<ParticipantsMotion>(buf).map(Packet::ParticipantsMotion)
        }
        PacketType::ParticipantsInfo => {
            read_unaligned::<ParticipantsInfo>(buf).map(Packet::ParticipantsInfo)
        }
        PacketType::ParticipantsDamage => {
            read_unaligned::<ParticipantsDamage>(buf).map(Packet::ParticipantsDamage)
        }
    }
}

fn read_unaligned<T: Copy>(buf: &[u8]) -> Option<T> {
    if buf.len() < std::mem::size_of::<T>() {
        return None;
    }
    Some(unsafe { ptr::read_unaligned(buf.as_ptr() as *const T) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_header() {
        // Cross-checked against TelemetryDataFormatPino.h.
        assert_eq!(std::mem::size_of::<Header>(), 14);
        assert_eq!(std::mem::size_of::<Session>(), 208);
        assert_eq!(std::mem::size_of::<Orientation>(), 34);
        assert_eq!(std::mem::size_of::<Velocity>(), 36);
        assert_eq!(std::mem::size_of::<VelocityEssential>(), 4);
        assert_eq!(std::mem::size_of::<Assists>(), 8);
        assert_eq!(std::mem::size_of::<Chassis>(), 52);
        assert_eq!(std::mem::size_of::<Driveline>(), 24);
        assert_eq!(std::mem::size_of::<Engine>(), 56);
        assert_eq!(std::mem::size_of::<Input>(), 20);
        assert_eq!(std::mem::size_of::<Tire>(), 72);
        assert_eq!(std::mem::size_of::<CarFull>(), 532);
        assert_eq!(std::mem::size_of::<InputExtended>(), 20);
        assert_eq!(std::mem::size_of::<Leaderboard>(), 32);
        assert_eq!(std::mem::size_of::<Timing>(), 32);
        assert_eq!(std::mem::size_of::<TimingSectors>(), 36);
        assert_eq!(std::mem::size_of::<ParticipantMotion>(), 38);
        assert_eq!(std::mem::size_of::<Damage>(), 21);
        assert_eq!(std::mem::size_of::<Info>(), 213);
    }

    #[test]
    fn rpm_offsets_match_legacy_constants() {
        // The pre-rewrite parser used these absolute offsets within a Main
        // packet to read engine RPM. Verify the new struct layout puts
        // those same fields at the same offsets.
        let main_base = std::mem::offset_of!(Main, car);
        let engine_base = main_base + std::mem::offset_of!(CarFull, engine);
        let rpm_offset = engine_base + std::mem::offset_of!(Engine, rpm);
        let redline_offset = engine_base + std::mem::offset_of!(Engine, rpm_redline);
        let idle_offset = engine_base + std::mem::offset_of!(Engine, rpm_idle);
        assert_eq!(rpm_offset, 350 + 84 + 1);
        assert_eq!(redline_offset, rpm_offset + 8);
        assert_eq!(idle_offset, rpm_offset + 12);
    }

    #[test]
    fn parse_returns_none_on_wrong_signature() {
        let mut buf = vec![0u8; std::mem::size_of::<Main>()];
        buf[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn parse_returns_none_on_wrong_packet_type_for_main_buf() {
        // A Main-sized buffer with the right signature but a different
        // packet_type byte parses as that other type, not as Main.
        let mut buf = vec![0u8; std::mem::size_of::<Main>()];
        buf[0..4].copy_from_slice(&SIGNATURE.to_le_bytes());
        buf[4] = PacketType::ParticipantsTiming as u8;
        let pkt = parse(&buf);
        assert!(!matches!(pkt, Some(Packet::Main(_))));
    }

    #[test]
    fn parse_returns_none_on_short_buffer() {
        let buf = [0u8; 5];
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn parse_main_extracts_engine_rpm() {
        let mut buf = vec![0u8; std::mem::size_of::<Main>()];
        buf[0..4].copy_from_slice(&SIGNATURE.to_le_bytes());
        buf[4] = PacketType::Main as u8;
        let engine_base = std::mem::offset_of!(Main, car) + std::mem::offset_of!(CarFull, engine);
        let rpm_off = engine_base + std::mem::offset_of!(Engine, rpm);
        let redline_off = engine_base + std::mem::offset_of!(Engine, rpm_redline);
        let idle_off = engine_base + std::mem::offset_of!(Engine, rpm_idle);
        buf[rpm_off..rpm_off + 4].copy_from_slice(&4200i32.to_le_bytes());
        buf[redline_off..redline_off + 4].copy_from_slice(&7000i32.to_le_bytes());
        buf[idle_off..idle_off + 4].copy_from_slice(&800i32.to_le_bytes());

        let main = match parse(&buf).expect("should parse") {
            Packet::Main(m) => m,
            _ => panic!("expected Main packet"),
        };
        let engine = main.car.engine;
        assert_eq!({ engine.rpm }, 4200);
        assert_eq!({ engine.rpm_redline }, 7000);
        assert_eq!({ engine.rpm_idle }, 800);
    }

    #[test]
    fn damage_unpack_recovers_packed_states() {
        // Pack one state per part: i % 5 (cycles through 0..=4).
        let mut comp = [0u8; DAMAGE_BYTES_PER_PARTICIPANT];
        for i in 0..damage_part::COUNT {
            let state = (i % 5) as u8;
            let bit_pos = i * DAMAGE_BITS_PER_PART;
            let byte_index = bit_pos / 8;
            let bit_offset = bit_pos % 8;
            comp[byte_index] |= state << bit_offset;
            if bit_offset > 5 && byte_index + 1 < comp.len() {
                comp[byte_index + 1] |= state >> (8 - bit_offset);
            }
        }
        let damage = Damage {
            damage_states: comp,
        };
        let states = damage.states();
        for (i, &got) in states.iter().enumerate() {
            assert_eq!(got, (i % 5) as u8, "part {i}");
        }
    }

    #[test]
    fn bitflags_contains_works() {
        let f = MarshalFlags::GREEN.bits() | MarshalFlags::BLUE.bits();
        let combined = MarshalFlags(f);
        assert!(combined.contains(MarshalFlags::GREEN));
        assert!(combined.contains(MarshalFlags::BLUE));
        assert!(!combined.contains(MarshalFlags::DQ));
    }
}
