//! Narrow Resolve-to-Resolve Fairlight preparation for the turnover archive.
use crate::{
    source::mixer::{valid_ticks, DIRECT_PROFILE, FAIRLIGHT_ADAPTER, FAIRLIGHT_LOSS},
    Result,
};
use graph::{
    source::{SourceGraph, SourceSequence},
    Loss, Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DirectMixer {
    pub(crate) buses: i64,
    pub(crate) pan: [i32; 2],
    pub(crate) pan_bypassed: [bool; 2],
    pub(crate) soloed: [bool; 2],
    pub(crate) output: [i32; 2],
    pub(crate) bus_2_gain: i32,
    pub(crate) bus_2_muted: bool,
    pub(crate) bus_pre_gain: [i32; 2],
    pub(crate) processing: [StripProcessing; 2],
    pub(crate) bus_processing: [StripProcessing; 2],
}

fn valid_dynamics_knob(value: i32) -> bool {
    matches!(value, 0..=100 | 5000..=100_094)
}

pub(crate) const PROCESSING_RECEIPT: &str = "qualified_track_processing";

pub(crate) const PROCESSING_CELL_COUNT: usize = 66;
pub(crate) type ProcessingCells = [i32; PROCESSING_CELL_COUNT];

#[derive(Clone, Copy)]
enum CellKind {
    Switch,
    Integer(i32, i32),
    Knob,
    EqType(usize),
    EqHz(usize),
    EqRange(usize),
    EqKnob(usize),
}

impl CellKind {
    fn accepts_scalar(self, value: i32) -> bool {
        match self {
            Self::Switch => matches!(value, 0 | 1),
            Self::Integer(min, max) => (min..=max).contains(&value),
            Self::Knob => valid_dynamics_knob(value),
            _ => false,
        }
    }
}

pub(crate) struct ProcessingRow {
    pub(crate) offset: usize,
    pub(crate) id: u32,
    name: &'static str,
    default: i32,
    kind: CellKind,
}

impl ProcessingRow {
    const fn new(offset: usize, id: u32, name: &'static str, default: i32, kind: CellKind) -> Self {
        Self {
            offset,
            id,
            name,
            default,
            kind,
        }
    }

    pub(crate) fn bus_ordinal(&self) -> Option<usize> {
        Some(match self.id {
            0xde..=0x112 => (self.id - 0xde) as usize,
            0x117 => 53,
            0x11d..=0x121 => 54 + (self.id - 0x11d) as usize,
            0x289..=0x28c => 162 + (self.id - 0x289) as usize,
            0x334..=0x339 => 174 + (self.id - 0x334) as usize,
            0x4be => 248,
            0x4c3 => 253,
            _ => return None,
        })
    }

    fn bus_control<'a>(&self, bus: &'a Value, bands: &[Option<&'a Value>; 6]) -> Option<&'a Value> {
        let id = self.id;
        let (band, name) = match id {
            0xde => return bus.get("processing_order_code"),
            0xdf => return bus.get("eq_enabled_code"),
            0xe1..=0xe6 => (id - 0xe1, "gain_tenths_db"),
            0xe9..=0xee => (id - 0xe9, "raw_q"),
            0xf1..=0xf6 => (id - 0xf1, "frequency_hz"),
            0xf9..=0xfe => (id - 0xf9, "filter_type_code"),
            0x101..=0x106 => (id - 0x101, "enabled_code"),
            0x289..=0x28c => (id - 0x288, "range_code"),
            0x334..=0x339 => (id - 0x334, "raw_frequency_knob"),
            0x4be => (0, "slope_code"),
            0x4c3 => (5, "slope_code"),
            _ => {
                let name = match id {
                    0x10c => "compressor_enabled_code",
                    0x114 => "expander_enabled_code",
                    0x11d => "limiter_enabled_code",
                    _ => self.name,
                };
                return bus.get("dynamics")?.get(name);
            }
        };
        bands[band as usize]?.get(name)
    }

    fn saved_key(&self) -> String {
        format!("0x{:03x}", self.id)
    }

    fn in_original_receipt(&self) -> bool {
        matches!(self.id, 0x0e3 | 0x10c..=0x10e)
    }

    fn is_switch(&self) -> bool {
        matches!(self.kind, CellKind::Switch)
    }
}

// Sorted by native offset for fingerprinting. Frequency, range and knob
// position are three saved fields for the same band frequency control.
pub(crate) const PROCESSING_ROWS: [ProcessingRow; PROCESSING_CELL_COUNT] = {
    use CellKind::*;
    [
        ProcessingRow::new(207_069, 0x0de, "processing_order_code", 5, Integer(0, 5)),
        ProcessingRow::new(207_089, 0x0df, "eq_enabled", 1, Switch),
        ProcessingRow::new(
            207_129,
            0x0e1,
            "eq_band1_gain_tenths_db",
            0,
            Integer(-200, 200),
        ),
        ProcessingRow::new(
            207_149,
            0x0e2,
            "eq_band2_gain_tenths_db",
            0,
            Integer(-200, 200),
        ),
        ProcessingRow::new(207_169, 0x0e3, "bell_gain_tenths_db", 0, Integer(-200, 200)),
        ProcessingRow::new(
            207_189,
            0x0e4,
            "eq_band4_gain_tenths_db",
            0,
            Integer(-200, 200),
        ),
        ProcessingRow::new(
            207_209,
            0x0e5,
            "eq_band5_gain_tenths_db",
            0,
            Integer(-200, 200),
        ),
        ProcessingRow::new(
            207_229,
            0x0e6,
            "eq_band6_gain_tenths_db",
            0,
            Integer(-200, 200),
        ),
        ProcessingRow::new(207_289, 0x0e9, "eq_band1_q_raw", 26, Integer(14, 166)),
        ProcessingRow::new(207_309, 0x0ea, "eq_band2_q_raw", 45, Integer(14, 166)),
        ProcessingRow::new(207_329, 0x0eb, "bell_q_raw", 26, Integer(14, 166)),
        ProcessingRow::new(207_349, 0x0ec, "eq_band4_q_raw", 26, Integer(14, 166)),
        ProcessingRow::new(207_369, 0x0ed, "eq_band5_q_raw", 45, Integer(14, 166)),
        ProcessingRow::new(207_389, 0x0ee, "eq_band6_q_raw", 26, Integer(14, 166)),
        ProcessingRow::new(207_449, 0x0f1, "eq_band1_frequency_hz", 70, EqHz(0)),
        ProcessingRow::new(207_469, 0x0f2, "eq_band2_frequency_hz", 119, EqHz(1)),
        ProcessingRow::new(207_489, 0x0f3, "bell_frequency_hz", 240, EqHz(2)),
        ProcessingRow::new(207_509, 0x0f4, "eq_band4_frequency_hz", 1223, EqHz(3)),
        ProcessingRow::new(207_529, 0x0f5, "eq_band5_frequency_hz", 6048, EqHz(4)),
        ProcessingRow::new(207_549, 0x0f6, "eq_band6_frequency_hz", 22000, EqHz(5)),
        ProcessingRow::new(207_609, 0x0f9, "eq_band1_filter_type_code", 6, EqType(0)),
        ProcessingRow::new(207_629, 0x0fa, "eq_band2_filter_type_code", 1, EqType(1)),
        ProcessingRow::new(207_649, 0x0fb, "eq_band3_filter_type_code", 2, EqType(2)),
        ProcessingRow::new(207_669, 0x0fc, "eq_band4_filter_type_code", 2, EqType(3)),
        ProcessingRow::new(207_689, 0x0fd, "eq_band5_filter_type_code", 3, EqType(4)),
        ProcessingRow::new(207_709, 0x0fe, "eq_band6_filter_type_code", 5, EqType(5)),
        ProcessingRow::new(207_769, 0x101, "eq_band1_enabled", 0, Switch),
        ProcessingRow::new(207_789, 0x102, "eq_band2_enabled", 1, Switch),
        ProcessingRow::new(207_809, 0x103, "eq_band3_enabled", 1, Switch),
        ProcessingRow::new(207_829, 0x104, "eq_band4_enabled", 1, Switch),
        ProcessingRow::new(207_849, 0x105, "eq_band5_enabled", 1, Switch),
        ProcessingRow::new(207_869, 0x106, "eq_band6_enabled", 0, Switch),
        ProcessingRow::new(207_929, 0x109, "makeup_tenths_db", 0, Integer(0, 120)),
        ProcessingRow::new(207_989, 0x10c, "compressor_enabled", 0, Switch),
        ProcessingRow::new(208_009, 0x10d, "compressor_ratio_raw", 30, Knob),
        ProcessingRow::new(
            208_029,
            0x10e,
            "compressor_threshold_tenths_db",
            -150,
            Integer(-600, 0),
        ),
        ProcessingRow::new(208_049, 0x10f, "compressor_knee_raw", 0, Integer(0, 100)),
        ProcessingRow::new(208_069, 0x110, "compressor_attack_raw", 20, Knob),
        ProcessingRow::new(208_089, 0x111, "compressor_hold_raw", 0, Knob),
        ProcessingRow::new(208_109, 0x112, "compressor_release_raw", 20, Knob),
        ProcessingRow::new(208_129, 0x114, "expander_enabled", 0, Switch),
        ProcessingRow::new(208_149, 0x115, "expander_range_raw", 30, Integer(0, 100)),
        ProcessingRow::new(
            208_169,
            0x116,
            "expander_threshold_raw",
            -350,
            Integer(-600, 0),
        ),
        ProcessingRow::new(208_189, 0x117, "compressor_mix_raw", 0, Integer(0, 100)),
        ProcessingRow::new(208_209, 0x118, "expander_attack_raw", 20, Knob),
        ProcessingRow::new(208_229, 0x119, "expander_hold_raw", 0, Knob),
        ProcessingRow::new(208_249, 0x11a, "expander_release_raw", 20, Knob),
        // Resolve 21.2.0 saved and rendered code 1 as gate on this profile.
        // Other nonzero codes remain unqualified.
        ProcessingRow::new(208_269, 0x11b, "expander_mode_code", 0, Integer(0, 1)),
        ProcessingRow::new(208_289, 0x11d, "limiter_enabled", 0, Switch),
        ProcessingRow::new(
            208_309,
            0x11e,
            "limiter_threshold_tenths_db",
            -210,
            Integer(-600, 0),
        ),
        ProcessingRow::new(208_329, 0x11f, "limiter_attack_raw", 20, Knob),
        ProcessingRow::new(208_349, 0x120, "limiter_hold_raw", 0, Knob),
        ProcessingRow::new(208_369, 0x121, "limiter_release_raw", 20, Knob),
        ProcessingRow::new(210_349, 0x253, "expander_ratio_raw", 0, Knob),
        ProcessingRow::new(210_429, 0x289, "eq_band2_frequency_range", 0, EqRange(1)),
        ProcessingRow::new(210_449, 0x28a, "bell_frequency_range", 1, EqRange(2)),
        ProcessingRow::new(210_469, 0x28b, "eq_band4_frequency_range", 2, EqRange(3)),
        ProcessingRow::new(210_489, 0x28c, "eq_band5_frequency_range", 3, EqRange(4)),
        ProcessingRow::new(211_073, 0x334, "eq_band1_frequency_knob", 85, EqKnob(0)),
        ProcessingRow::new(211_093, 0x335, "eq_band2_frequency_knob", 137, EqKnob(1)),
        ProcessingRow::new(211_113, 0x336, "bell_frequency_knob", 83, EqKnob(2)),
        ProcessingRow::new(211_133, 0x337, "eq_band4_frequency_knob", 89, EqKnob(3)),
        ProcessingRow::new(211_153, 0x338, "eq_band5_frequency_knob", 136, EqKnob(4)),
        ProcessingRow::new(211_173, 0x339, "eq_band6_frequency_knob", 256, EqKnob(5)),
        ProcessingRow::new(212_733, 0x4be, "eq_band1_slope_code", 1, Integer(0, 3)),
        ProcessingRow::new(212_833, 0x4c3, "eq_band6_slope_code", 1, Integer(0, 3)),
    ]
};

const fn row_defaults<const N: usize>(rows: &[ProcessingRow; N]) -> [i32; N] {
    let mut defaults = [0; N];
    let mut index = 0;
    while index < N {
        defaults[index] = rows[index].default;
        index += 1;
    }
    defaults
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StripProcessing(ProcessingCells);

impl StripProcessing {
    pub(crate) const DEFAULT: Self = Self(row_defaults(&PROCESSING_ROWS));

    pub(crate) fn from_cells(cells: ProcessingCells) -> Option<Self> {
        let mut frequencies = [0; 6];
        let mut ranges = [0, -1, -1, -1, -1, 3];
        let mut knobs = [-1; 6];
        for (row, value) in PROCESSING_ROWS.iter().zip(cells) {
            match row.kind {
                kind if kind.accepts_scalar(value) => {}
                CellKind::EqType(band) if EQ_TYPES[band].contains(&value) => {}
                CellKind::EqHz(band) => frequencies[band] = value,
                CellKind::EqRange(band) => ranges[band] = value,
                CellKind::EqKnob(band) => knobs[band] = value,
                _ => return None,
            }
        }
        (0..6)
            .all(|band| {
                let (hz, range, knob) = (frequencies[band], ranges[band], knobs[band]);
                // These two untouched factory tuples differ from the
                // edited-value law by more than half a knob step.
                matches!((band, hz, range, knob), (0, 70, 0, 85) | (1, 119, 0, 137))
                    || frequency_agrees(hz, range, knob)
            })
            .then_some(Self(cells))
    }

    pub(crate) const fn cells(self) -> ProcessingCells {
        self.0
    }

    pub(crate) fn receipt(self) -> Value {
        Value::Obj(
            PROCESSING_ROWS
                .iter()
                .zip(self.0)
                .map(|(row, value)| {
                    let value = if row.is_switch() {
                        Value::Bool(value == 1)
                    } else {
                        Value::Int(value.into())
                    };
                    (row.name.into(), value)
                })
                .collect(),
        )
    }

    pub(crate) fn from_bus_receipt(bus: &Value) -> Result<Self> {
        let mut bands = [None; 6];
        match bus.get("eq_bands") {
            None => {}
            Some(Value::Arr(rows)) => {
                for band in rows {
                    let index = match band.get("band") {
                        Some(Value::Int(n @ 1..=6)) => (*n - 1) as usize,
                        _ => return Err("invalid Fairlight bus EQ band index".into()),
                    };
                    if bands[index].replace(band).is_some() {
                        return Err("duplicate Fairlight bus EQ band".into());
                    }
                }
            }
            _ => return Err("invalid Fairlight bus EQ bands".into()),
        }
        let mut cells = Self::DEFAULT.cells();
        for (row, cell) in PROCESSING_ROWS.iter().zip(&mut cells) {
            let raw = bus
                .get("eq_dynamics_fields")
                .and_then(|values| values.get(&row.saved_key()));
            let control = row.bus_control(bus, &bands);
            if row.bus_ordinal().is_none() && (raw.is_some() || control.is_some()) {
                return Err(format!("unsupported Fairlight bus control {}", row.name).into());
            }
            match (raw, control) {
                (None, None) => {}
                (Some(Value::Int(raw)), Some(Value::Int(control))) if raw == control => {
                    *cell = i32::try_from(*raw)?;
                }
                _ => {
                    return Err(format!("bus control {} disagrees with saved row", row.name).into())
                }
            }
        }
        Self::from_cells(cells).ok_or_else(|| "unsupported Fairlight bus processing values".into())
    }

    pub(crate) fn saved_rows_are_default(track: &Value) -> bool {
        let Some(rows) = track.get("eq_dynamics_fields") else {
            // Earlier receipts can omit raw processing observations.
            return true;
        };
        PROCESSING_ROWS
            .iter()
            .zip(Self::DEFAULT.cells())
            .all(|(row, default)| match rows.get(&row.saved_key()) {
                None => true,
                Some(Value::Int(value)) => *value == i64::from(default),
                _ => false,
            })
    }

    /// Archive comparisons need positive evidence of each supported saved
    /// cell. The writer also accepts older receipts that omit default cells.
    pub(crate) fn observes_saved_cells(strip: &Value, bus: bool) -> bool {
        let Some(rows) = strip.get("eq_dynamics_fields") else {
            return false;
        };
        PROCESSING_ROWS.iter().filter(|row| !bus || row.bus_ordinal().is_some()).all(|row| {
            matches!(rows.get(&row.saved_key()), Some(Value::Int(_)))
        })
    }

    pub(crate) fn from_receipt(value: &Value, track: &Value) -> Result<Self> {
        let raw = track
            .get("eq_dynamics_fields")
            .ok_or("missing saved Fairlight processing rows")?;
        let mut cells = Self::DEFAULT.cells();
        for (row, cell) in PROCESSING_ROWS.iter().zip(&mut cells) {
            let field = value.get(row.name);
            let saved = raw.get(&row.saved_key());
            // Older receipts may omit a later control at its native default.
            if field.is_none() && !row.in_original_receipt() {
                if saved.is_none() || saved == Some(&Value::Int(i64::from(*cell))) {
                    continue;
                }
                return Err(
                    format!("missing qualified Fairlight processing field {}", row.name).into(),
                );
            }
            *cell = match (row.is_switch(), field) {
                (true, Some(Value::Bool(enabled))) => i32::from(*enabled),
                (false, Some(Value::Int(value))) => i32::try_from(*value)?,
                _ => {
                    return Err(format!(
                        "invalid qualified Fairlight processing field {}",
                        row.name
                    )
                    .into())
                }
            };
            if saved != Some(&Value::Int(i64::from(*cell))) {
                return Err(format!(
                    "qualified Fairlight processing disagrees with saved {}",
                    row.name
                )
                .into());
            }
        }
        Self::from_cells(cells)
            .ok_or_else(|| "unsupported Fairlight track processing values".into())
    }
}

// The middle bands share the shelf, peak and notch modes. The outer
// bands replace peak with a high-pass or low-pass filter respectively.
const EQ_TYPES: [&[i32]; 6] = [
    &[1, 3, 4, 6],
    &[1, 2, 3, 4],
    &[1, 2, 3, 4],
    &[1, 2, 3, 4],
    &[1, 2, 3, 4],
    &[1, 3, 4, 5],
];

fn frequency_agrees(hz: i32, range: i32, knob: i32) -> bool {
    let (min, max, log_max) = match range {
        0 => (30, 399, 400),
        1 => (100, 1500, 1500),
        2 => (450, 8000, 8000),
        3 => (1400, 22000, 22000),
        _ => return false,
    };
    if !(min..=max).contains(&hz) || !(0..=256).contains(&knob) {
        return false;
    }
    let min = f64::from(min);
    let position = 256.0 * (f64::from(hz) / min).ln() / (f64::from(log_max) / min).ln();
    (position - f64::from(knob)).abs() <= 0.5 + 1e-8
}

impl DirectMixer {
    pub(crate) fn recover(loss: &Loss, owner: &str) -> bool {
        loss.object == owner && loss.aspect == FAIRLIGHT_LOSS.0 && loss.reason == FAIRLIGHT_LOSS.1
    }

    pub(crate) fn prepare(graph: &SourceGraph, sequence: &SourceSequence) -> Result<Option<Self>> {
        let Some(data) = graph.sidecar(FAIRLIGHT_ADAPTER, &sequence.id) else {
            return Ok(None);
        };
        if data.get(DIRECT_PROFILE).is_none() {
            if data.get(PROCESSING_RECEIPT).is_some() {
                return Err(
                    "qualified track processing requires a direct Fairlight mixer profile".into(),
                );
            }
            // Unqualified raw observations retain their original source loss.
            return Ok(None);
        }
        if !sequence
            .clock
            .as_ref()
            .is_some_and(|clock| clock.provenance.route == "Resolve archive clock")
        {
            return Err("direct Fairlight controls require a native Resolve clock".into());
        }
        let mixer = Self::from_receipt(data)?;
        if mixer.buses == 1
            && mixer.pan == [0; 2]
            && mixer.pan_bypassed == [false; 2]
            && mixer.soloed == [false; 2]
            && mixer.processing == [StripProcessing::DEFAULT; 2]
            && mixer.bus_processing == [StripProcessing::DEFAULT; 2]
            && mixer.bus_pre_gain == [0; 2]
        {
            // Keep neutral legacy output unchanged.
            return Ok(None);
        }
        Ok(Some(mixer))
    }

    fn from_receipt(data: &Value) -> Result<Self> {
        let buses = number(data, DIRECT_PROFILE)?;
        if data.get("plugins").is_some() {
            return Err("direct Fairlight profile does not map mixer plug-ins".into());
        }
        let Some(Value::Arr(outputs)) = data.get("buses") else {
            return Err("direct Fairlight mixer is missing its persisted outputs".into());
        };
        if !matches!(buses, 1 | 2) || outputs.len() as i64 != buses {
            return Err(
                "direct Fairlight mixer requires one or two persisted stereo outputs".into(),
            );
        }
        let mut bus_2_gain = 0;
        let mut bus_2_muted = false;
        let mut bus_pre_gain = [0; 2];
        let mut bus_processing = [StripProcessing::DEFAULT; 2];
        for (index, bus) in (0i64..).zip(outputs) {
            if number(bus, "index")? != index
                || number(bus, "strip_id")? != 3 + index
                || number(bus, "channels")? != 2
                || number(bus, "output_connected_code")? != 0
                || number(bus, "output_bus_index")? != -1
                || !neutral_when_present(
                    bus,
                    &["raw_pan", "raw_pan_bypass", "raw_solo", "effects_in_code"],
                )
            {
                return Err("unsupported or ambiguous direct Fairlight stereo bus".into());
            }
            let gain = number(bus, "fader_tenths_db")?;
            let mute = number(bus, "mute_code")?;
            bus_pre_gain[index as usize] = match bus.get("pre_dynamics_gain_tenths_db") {
                None => 0,
                Some(Value::Int(value @ -60..=60)) => *value as i32,
                _ => return Err("unsupported direct Fairlight bus input gain".into()),
            };
            bus_processing[index as usize] = StripProcessing::from_bus_receipt(bus)?;
            if index == 0 {
                if gain != 0 || mute != 0 {
                    return Err(
                        "direct Fairlight stereo Bus 1 requires neutral gain and mute".into(),
                    );
                }
            } else {
                bus_2_gain = i32::try_from(gain)
                    .ok()
                    .filter(|&ticks| valid_ticks(ticks))
                    .ok_or("unsupported direct Fairlight Bus 2 gain")?;
                if !matches!(mute, 0 | 1) {
                    return Err("unsupported direct Fairlight Bus 2 mute code".into());
                }
                bus_2_muted = mute == 1;
            }
        }
        if data.get("main_bus") != outputs.first() {
            return Err("direct Fairlight main bus receipt disagrees with bus identity".into());
        }
        let Some(Value::Arr(tracks)) = data.get("tracks") else {
            return Err("direct Fairlight mixer is missing its two mono tracks".into());
        };
        if tracks.len() != 2 {
            return Err("direct Fairlight mixer requires two independent mono tracks".into());
        }
        let mut pan = [0; 2];
        let mut pan_bypassed = [false; 2];
        let mut soloed = [false; 2];
        let mut output = [0; 2];
        for (column, row) in tracks.iter().enumerate() {
            let position = number(row, "pan_tenths_percent")?;
            let bus = number(row, "output_bus_index")?;
            let bypass = number(row, "pan_bypass_code")?;
            soloed[column] = match row.get("solo_code") {
                None | Some(Value::Int(0)) => false,
                Some(Value::Int(1)) if buses == 1 => true,
                _ => return Err("unsupported direct Fairlight track solo".into()),
            };
            if row
                .get("unresolved_channel_fields")
                .and_then(|fields| fields.get("0x011_second"))
                .is_some_and(|raw| raw != &Value::Int(0))
            {
                return Err("unsupported secondary Fairlight solo row".into());
            }
            if number(row, "track")? != column as i64 + 1
                || number(row, "strip_id")? != column as i64
                || number(row, "channel_index")? != 0
                || !(bypass == 0 || (bypass == 1 && buses == 1))
                || number(row, "output_connected_code")? != 1
                || number(row, "fader_tenths_db")? != 0
                || number(row, "mute_code")? != 0
                || !neutral_when_present(row, &["pre_dynamics_gain_tenths_db", "effects_in_code"])
                || !(-1000..=1000).contains(&position)
                || !(0..buses).contains(&bus)
            {
                return Err("unsupported direct Fairlight mono pan or output assignment".into());
            }
            pan[column] = position as i32;
            pan_bypassed[column] = bypass == 1;
            output[column] = bus as i32;
        }
        let processing = match data.get(PROCESSING_RECEIPT) {
            None if tracks.iter().all(StripProcessing::saved_rows_are_default) => {
                [StripProcessing::DEFAULT; 2]
            }
            None => return Err("saved Fairlight processing lacks a qualified track receipt".into()),
            Some(Value::Arr(rows)) if rows.len() == 2 => [
                StripProcessing::from_receipt(&rows[0], &tracks[0])?,
                StripProcessing::from_receipt(&rows[1], &tracks[1])?,
            ],
            _ => {
                return Err("expected one Fairlight processing receipt for each mono track".into())
            }
        };
        Ok(Self {
            buses,
            pan,
            pan_bypassed,
            soloed,
            output,
            bus_2_gain,
            bus_2_muted,
            bus_pre_gain,
            processing,
            bus_processing,
        })
    }
}

fn number(row: &Value, name: &str) -> Result<i64> {
    match row.get(name) {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(format!("direct Fairlight receipt is missing integer {name}").into()),
    }
}

pub(crate) fn neutral_when_present(row: &Value, names: &[&str]) -> bool {
    names
        .iter()
        .all(|name| matches!(row.get(name), None | Some(Value::Int(0))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value as Json};

    fn receipt() -> Json {
        let bus = |index| {
            json!({
                "index": index, "strip_id": 3 + index, "channels": 2,
                "output_connected_code": 0, "output_bus_index": -1,
                "fader_tenths_db": 0, "mute_code": 0
            })
        };
        let track = |strip, pan, output| {
            json!({
                "track": 1 + strip, "strip_id": strip, "channel_index": 0,
                "pan_tenths_percent": pan, "pan_bypass_code": 0, "output_connected_code": 1,
                "output_bus_index": output, "fader_tenths_db": 0, "mute_code": 0
            })
        };
        json!({
            (DIRECT_PROFILE): 2, "buses": [bus(0), bus(1)], "main_bus": bus(0),
            "tracks": [track(0, -1000, 0), track(1, 500, 1)]
        })
    }

    fn prepare(value: Json) -> Result<DirectMixer> {
        DirectMixer::from_receipt(&graph::json::parse(&value.to_string())?)
    }

    #[test]
    fn bus_input_gain_keeps_its_bus_and_is_independent_of_the_fader() {
        let mut saved = receipt();
        saved["buses"][0]["pre_dynamics_gain_tenths_db"] = json!(30);
        saved["buses"][1]["pre_dynamics_gain_tenths_db"] = json!(-60);
        saved["buses"][1]["fader_tenths_db"] = json!(-20);
        saved["main_bus"] = saved["buses"][0].clone();
        let mixer = prepare(saved.clone()).unwrap();
        assert_eq!(mixer.bus_pre_gain, [30, -60]);
        assert_eq!(mixer.bus_2_gain, -20);

        saved["buses"][1]["pre_dynamics_gain_tenths_db"] = json!(-61);
        assert!(prepare(saved)
            .unwrap_err()
            .to_string()
            .contains("unsupported direct Fairlight bus input gain"));
    }

    #[test]
    fn bus_dynamics_follow_bus_index_and_require_matching_rows() {
        let mut saved = receipt();
        saved["buses"][0]["eq_dynamics_fields"] =
            json!({"0x11d":1,"0x11e":-420,"0x11f":35,"0x120":16496,"0x121":45});
        saved["buses"][0]["dynamics"] = json!({
            "limiter_enabled_code":1,"limiter_threshold_tenths_db":-420,
            "limiter_attack_raw":35,"limiter_hold_raw":16496,"limiter_release_raw":45
        });
        saved["buses"][1]["eq_dynamics_fields"] = json!({"0x10c":1,"0x10d":64996,"0x10e":-450});
        saved["buses"][1]["dynamics"] = json!({
            "compressor_enabled_code":1,"compressor_ratio_raw":64996,
            "compressor_threshold_tenths_db":-450
        });
        saved["main_bus"] = saved["buses"][0].clone();
        let mixer = prepare(saved.clone()).unwrap();
        assert_eq!(
            mixer.bus_processing.map(StripProcessing::cells),
            [
                cells_with(&[
                    (0x11d, 1),
                    (0x11e, -420),
                    (0x11f, 35),
                    (0x120, 16496),
                    (0x121, 45)
                ]),
                cells_with(&[(0x10c, 1), (0x10d, 64996), (0x10e, -450)]),
            ],
        );
        assert_eq!(mixer.processing, [StripProcessing::DEFAULT; 2]);
        saved["buses"][1]["dynamics"]["compressor_threshold_tenths_db"] = json!(-440);
        assert!(prepare(saved)
            .unwrap_err()
            .to_string()
            .contains("disagrees with saved row"));
    }

    #[test]
    fn bus_eq_binds_controls_to_band_and_bus() {
        let mut saved = receipt();
        saved["buses"][1]["eq_dynamics_fields"] = json!({"0x0e3":60,"0x0de":0});
        saved["buses"][1]["eq_bands"] = json!([{"band":3,"gain_tenths_db":60}]);
        saved["buses"][1]["processing_order_code"] = json!(0);
        assert_eq!(
            prepare(saved.clone())
                .unwrap()
                .bus_processing
                .map(StripProcessing::cells),
            [
                StripProcessing::DEFAULT.cells(),
                cells_with(&[(0xe3, 60), (0xde, 0)])
            ],
        );
        saved["buses"][1]["eq_bands"]
            .as_array_mut()
            .unwrap()
            .push(json!({"band":3,"gain_tenths_db":60}));
        assert!(prepare(saved.clone())
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        saved["buses"][1]["eq_bands"] = json!([{"band":4,"gain_tenths_db":60}]);
        assert!(prepare(saved)
            .unwrap_err()
            .to_string()
            .contains("disagrees with saved row"));
    }

    fn cells_with(changes: &[(u32, i32)]) -> ProcessingCells {
        let mut cells = StripProcessing::DEFAULT.cells();
        for &(id, value) in changes {
            cells[PROCESSING_ROWS.iter().position(|row| row.id == id).unwrap()] = value;
        }
        cells
    }

    #[test]
    fn processing_requires_valid_native_cells_and_agreement_by_track() {
        let mut saved = receipt();
        let tracks = saved["tracks"].as_array_mut().unwrap();
        tracks[0]["eq_dynamics_fields"] = json!({"0x0e3":60,"0x10c":0,"0x10d":30,"0x10e":-150});
        tracks[1]["eq_dynamics_fields"] = json!({"0x0e3":0,"0x10c":1,"0x10d":64996,"0x10e":-300});
        assert!(prepare(saved.clone()).is_err());
        saved[PROCESSING_RECEIPT] = json!([
            {"bell_gain_tenths_db":60,"compressor_enabled":false,"compressor_ratio_raw":30,"compressor_threshold_tenths_db":-150},
            {"bell_gain_tenths_db":0,"compressor_enabled":true,"compressor_ratio_raw":64996,"compressor_threshold_tenths_db":-300}
        ]);
        assert_eq!(
            prepare(saved.clone())
                .unwrap()
                .processing
                .map(StripProcessing::cells),
            [
                cells_with(&[(0xe3, 60)]),
                cells_with(&[(0x10c, 1), (0x10d, 64996), (0x10e, -300)])
            ]
        );
        saved[PROCESSING_RECEIPT][1]["compressor_ratio_raw"] = json!(4000);
        assert!(prepare(saved.clone()).is_err());
        saved["tracks"][1]["eq_dynamics_fields"]["0x10d"] = json!(4000);
        assert!(prepare(saved).is_err());
    }

    #[test]
    fn preserves_band_frequency_triplet_and_both_compressor_timing_encodings() {
        let controls = [
            (0xde, 0),
            (0xe3, 60),
            (0xeb, 42),
            (0xf3, 1223),
            (0x109, 60),
            (0x10c, 1),
            (0x10d, 59),
            (0x10e, -300),
            (0x10f, 50),
            (0x110, 40496),
            (0x111, 20),
            (0x112, 29996),
            (0x117, 50),
            (0x28a, 2),
            (0x336, 89),
        ];
        let valid = cells_with(&controls);
        assert_eq!(StripProcessing::from_cells(valid).unwrap().cells(), valid);
        for change in [
            (0xde, 6),
            (0xdf, 2),
            (0xeb, 167),
            (0x109, -1),
            (0x10f, 101),
            (0x110, 4000),
            (0x117, 101),
            (0x28a, 0),
            (0x336, 83),
        ] {
            let mut rows = controls.to_vec();
            rows.push(change);
            assert!(StripProcessing::from_cells(cells_with(&rows)).is_none());
        }
    }

    #[test]
    fn expander_and_limiter_require_supported_mode_and_saved_codes() {
        let values = [
            (0x114, 1),
            (0x115, 40),
            (0x116, -120),
            (0x118, 40496),
            (0x119, 25),
            (0x11a, 29996),
            (0x253, 80096),
            (0x11d, 1),
            (0x11e, -420),
            (0x11f, 45),
            (0x120, 16496),
            (0x121, 35),
        ];
        let cells = cells_with(&values);
        let processing = StripProcessing::from_cells(cells).unwrap();
        assert_eq!(processing.cells(), cells);
        assert_eq!(
            processing.receipt().get("expander_enabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            processing.receipt().get("limiter_enabled"),
            Some(&Value::Bool(true))
        );
        for change in [(0x11b, 2), (0x11f, 4000), (0x253, 101), (0x11d, 2)] {
            let mut rows = values.to_vec();
            rows.push(change);
            assert!(StripProcessing::from_cells(cells_with(&rows)).is_none());
        }
    }

    #[test]
    fn preserves_gate_mode_independently_of_the_enable_switch() {
        for enabled in [0, 1] {
            let cells = cells_with(&[(0x114, enabled), (0x11b, 1), (0x116, -210)]);
            let processing = StripProcessing::from_cells(cells).unwrap();
            assert_eq!(
                processing.receipt().get("expander_mode_code"),
                Some(&Value::Int(1))
            );
            assert_eq!(
                processing.receipt().get("expander_enabled"),
                Some(&Value::Bool(enabled == 1))
            );
        }
        assert!(StripProcessing::from_cells(cells_with(&[(0x11b, -1)])).is_none());
    }

    #[test]
    fn checks_frequency_per_band_and_preserves_outer_slopes() {
        let factory = StripProcessing::DEFAULT.cells();
        assert!(StripProcessing::from_cells(factory).is_some());
        let controls = [
            (0xf1, 140),
            (0x334, 152),
            (0x101, 1),
            (0x4be, 3),
            (0x106, 1),
            (0x4c3, 2),
        ];
        let settings = StripProcessing::from_cells(cells_with(&controls)).unwrap();
        assert_eq!(
            settings.receipt().get("eq_band1_enabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            settings.receipt().get("eq_band1_slope_code"),
            Some(&Value::Int(3))
        );
        assert_eq!(
            settings.receipt().get("eq_band6_slope_code"),
            Some(&Value::Int(2))
        );
        for invalid in [(0x334, 85), (0x28c, 0), (0x4be, 4), (0xf9, 5)] {
            let mut rows = controls.to_vec();
            rows.push(invalid);
            assert!(StripProcessing::from_cells(cells_with(&rows)).is_none());
        }
    }

    #[test]
    fn missing_later_control_is_only_compatible_at_its_default() {
        let mut saved = receipt();
        let old = json!({
            "bell_gain_tenths_db":0,"compressor_enabled":false,
            "compressor_ratio_raw":30,"compressor_threshold_tenths_db":-150
        });
        saved[PROCESSING_RECEIPT] = json!([old, old]);
        for track in saved["tracks"].as_array_mut().unwrap() {
            track["eq_dynamics_fields"] =
                json!({"0x0e3":0,"0x10c":0,"0x10d":30,"0x10e":-150,"0x117":0});
        }
        assert!(prepare(saved.clone()).is_ok());
        saved["tracks"][1]["eq_dynamics_fields"]["0x117"] = json!(50);
        assert!(prepare(saved.clone())
            .unwrap_err()
            .to_string()
            .contains("missing qualified Fairlight processing field compressor_mix_raw"));
        saved[PROCESSING_RECEIPT][1]["compressor_mix_raw"] = json!(50);
        let processing = prepare(saved.clone()).unwrap().processing;
        assert_eq!(processing[0], StripProcessing::DEFAULT);
        assert_eq!(
            processing[1].receipt().get("compressor_mix_raw"),
            Some(&Value::Int(50))
        );
        saved[PROCESSING_RECEIPT][1]["compressor_mix_raw"] = json!(49);
        assert!(prepare(saved).is_err());
    }

    #[test]
    fn preserves_signed_pan_and_independent_direct_output_by_strip() {
        let prepared = prepare(receipt()).unwrap();
        assert_eq!(prepared.pan, [-1000, 500]);
        assert_eq!(prepared.pan_bypassed, [false; 2]);
        assert_eq!(prepared.output, [0, 1]);
    }

    #[test]
    fn single_bus_pan_bypass_preserves_the_saved_pan() {
        let mut saved = receipt();
        saved[DIRECT_PROFILE] = json!(1);
        saved["buses"].as_array_mut().unwrap().truncate(1);
        saved["tracks"][1]["output_bus_index"] = json!(0);
        saved["tracks"][0]["pan_bypass_code"] = json!(1);
        let mixer = prepare(saved.clone()).unwrap();
        assert_eq!(mixer.pan_bypassed, [true, false]);
        assert_eq!(mixer.pan, [-1000, 500]);
        saved["tracks"][0]["pan_bypass_code"] = json!(2);
        assert!(prepare(saved).is_err());
    }

    #[test]
    fn solo_selects_each_mono_strip_without_changing_pan_or_routing() {
        let mut saved = receipt();
        saved["tracks"][1]["solo_code"] = json!(1);
        assert!(prepare(saved.clone()).is_err());
        saved[DIRECT_PROFILE] = json!(1);
        saved["buses"].as_array_mut().unwrap().truncate(1);
        saved["tracks"][1]["output_bus_index"] = json!(0);
        for (a1, a2) in [(0, 1), (1, 0), (1, 1)] {
            saved["tracks"][0]["solo_code"] = json!(a1);
            saved["tracks"][1]["solo_code"] = json!(a2);
            let mixer = prepare(saved.clone()).unwrap();
            assert_eq!(mixer.soloed, [a1 == 1, a2 == 1]);
            assert_eq!(mixer.pan, [-1000, 500]);
            assert_eq!(mixer.output, [0, 0]);
        }
        saved["tracks"][0]["unresolved_channel_fields"] = json!({"0x011_second": 1});
        assert!(prepare(saved.clone()).is_err());
        saved["tracks"][0]["unresolved_channel_fields"]["0x011_second"] = json!(0);
        saved["tracks"][0]["solo_code"] = json!(2);
        assert!(prepare(saved).is_err());
    }

    #[test]
    fn refuses_ambiguous_missing_or_disconnected_outputs_and_unmapped_processing() {
        for (pointer, change) in [
            ("/buses/1/strip_id", json!(3)),
            ("/buses/1/channels", json!(1)),
            ("/buses/1/output_connected_code", json!(1)),
            ("/buses/0/fader_tenths_db", json!(-60)),
            ("/buses/0/mute_code", json!(1)),
            ("/buses/1/fader_tenths_db", json!(-61)),
            ("/buses/1/mute_code", json!(2)),
            ("/main_bus/index", json!(1)),
            ("/tracks/1/channel_index", json!(1)),
            ("/tracks/1/output_bus_index", json!(2)),
            ("/tracks/1/output_connected_code", json!(0)),
            ("/tracks/1/pan_bypass_code", json!(1)),
            ("/tracks/1/pan_tenths_percent", json!(1001)),
        ] {
            let mut value = receipt();
            *value.pointer_mut(pointer).unwrap() = change;
            assert!(prepare(value).is_err(), "accepted {pointer}");
        }
        let mut value = receipt();
        value["plugins"] = json!([{"track":1,"slot":6}]);
        assert!(prepare(value).is_err());
        let mut value = receipt();
        value["buses"].as_array_mut().unwrap().pop();
        assert!(prepare(value).is_err());
    }

    #[test]
    fn preserves_bus_2_gain_and_mute_independently_of_pan_and_routing() {
        for (gain, mute) in [(-60, 0), (0, 1), (-37, 1)] {
            let mut value = receipt();
            value["buses"][1]["fader_tenths_db"] = json!(gain);
            value["buses"][1]["mute_code"] = json!(mute);
            let prepared = prepare(value).unwrap();
            assert_eq!(
                (prepared.bus_2_gain, prepared.bus_2_muted),
                (gain, mute == 1)
            );
            assert_eq!(prepared.pan, [-1000, 500]);
            assert_eq!(prepared.output, [0, 1]);
        }
    }
}
