// Wreckfest 2 "Pino" telemetry packet parser.
//
// Schema is defined by Bugbear in TelemetryDataFormatPino.h shipped inside the
// game install dir. Structs use #pragma pack(1) so all fields are byte-tight.
//
// We only need the player engine state from the Main packet (packetType = 0).

const SIGNATURE: u32 = 1869769584; // "Pino" little-endian: 50 69 6E 6F
const PACKET_TYPE_MAIN: u8 = 0;

// Header is 14 bytes: signature(4) packetType(1) statusFlags(1) sessionTime(4) raceTime(4)
//
// Main layout (cumulative byte offsets):
//   header                          @   0  (14)
//   marshalFlagsPlayer: U16         @  14  ( 2)
//   Leaderboard                     @  16  (32)
//   Timing                          @  48  (32)
//   TimingSectors                   @  80  (36)
//   Info                            @ 116  (213)
//   Damage                          @ 329  (21)
//   Car::Full                       @ 350  (532)
//
// Within Car::Full:
//   Assists                         @   0  ( 8)
//   Chassis                         @   8  (52)
//   Driveline                       @  60  (24)
//   Engine                          @  84  (56)
//
// Within Engine:
//   flags: U8                       @   0
//   rpm: S32                        @   1
//   rpmMax: S32                     @   5  (limiter — unused, redline is more useful)
//   rpmRedline: S32                 @   9
//   rpmIdle: S32                    @  13
const ENGINE_RPM_OFFSET: usize = 350 + 84 + 1;
const ENGINE_RPM_REDLINE_OFFSET: usize = ENGINE_RPM_OFFSET + 8;
const ENGINE_RPM_IDLE_OFFSET: usize = ENGINE_RPM_OFFSET + 12;

#[derive(Debug, Clone, Copy)]
pub struct EngineState {
    pub rpm: i32,
    pub rpm_redline: i32,
    pub rpm_idle: i32,
}

pub fn parse_main(buf: &[u8]) -> Option<EngineState> {
    if buf.len() < ENGINE_RPM_IDLE_OFFSET + 4 {
        return None;
    }
    let signature = u32::from_le_bytes(buf[0..4].try_into().ok()?);
    if signature != SIGNATURE {
        return None;
    }
    let packet_type = buf[4];
    if packet_type != PACKET_TYPE_MAIN {
        return None;
    }
    Some(EngineState {
        rpm: read_i32(buf, ENGINE_RPM_OFFSET),
        rpm_redline: read_i32(buf, ENGINE_RPM_REDLINE_OFFSET),
        rpm_idle: read_i32(buf, ENGINE_RPM_IDLE_OFFSET),
    })
}

fn read_i32(buf: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}
