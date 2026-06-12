use core::marker::PhantomData;
use core::ptr;

use crate::swj::{ProbeIo, ReadBlockResult, TransferStatus, WriteBlockResult};
use hpm5301_hal::{Peri, pac, peripherals, time};

pub const PIN_TDO: u8 = 26;
pub const PIN_SWCLK_TCK: u8 = 27;
pub const PIN_SWDIO_TMS: u8 = 28;
pub const PIN_TDI: u8 = 29;
pub const PIN_NRESET: u8 = 10;
pub const PIN_ACTIVITY_LED: u8 = 10;

const PORT_A: usize = 0;
const PORT_B: usize = 1;
const FGPIO_BASE: usize = 0x000c_0000;
const DO_BASE: usize = 0x100;
const DO_SET_OFFSET: usize = DO_BASE + 4;
const DO_CLR_OFFSET: usize = DO_BASE + 8;
const OE_BASE: usize = 0x200;
const PAD_NRESET: u8 = 42;

const SWCLK_TCK: u32 = 1 << PIN_SWCLK_TCK;
const SWDIO_TMS: u32 = 1 << PIN_SWDIO_TMS;
const TDI: u32 = 1 << PIN_TDI;
const TDO: u32 = 1 << PIN_TDO;
const NRESET: u32 = 1 << PIN_NRESET;
const ACTIVITY_LED: u32 = 1 << PIN_ACTIVITY_LED;

const OUTPUT_MASK_A: u32 = SWCLK_TCK | SWDIO_TMS | TDI | ACTIVITY_LED;
const OUTPUT_MASK_B: u32 = NRESET;
const DEFAULT_SWJ_CLOCK_HZ: u32 = 8_000_000;
const MAX_STABLE_SWJ_CLOCK_HZ: u32 = 8_000_000;
const PROBE_RS_DEFAULT_SWJ_CLOCK_HZ: u32 = 1_000_000;
const ACTIVITY_LED_ACTIVE_HIGH: bool = false;
const ACTIVITY_LED_MODE: ActivityLedMode = ActivityLedMode::BlinkBusy;
const ACTIVITY_LED_BLINK_TOGGLE_MS: u64 = 100;
const ACTIVITY_LED_POLL_INTERVAL: u8 = 64;

pub struct ProbePins {
    half_period_delay: u8,
    write_half_period_delay: u8,
    swdio_output_enabled: bool,
    output_a: u32,
    output_b: u32,
    activity_led_active: bool,
    activity_led_lit: bool,
    activity_led_next_toggle: u64,
    activity_led_poll_count: u8,
    _owned: PhantomData<(
        peripherals::PA26,
        peripherals::PA27,
        peripherals::PA28,
        peripherals::PA29,
        peripherals::PB10,
        peripherals::PA10,
    )>,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
enum ActivityLedMode {
    SteadyBusy,
    BlinkBusy,
}

impl ProbePins {
    pub fn new(
        _tdo: Peri<'static, peripherals::PA26>,
        _swclk_tck: Peri<'static, peripherals::PA27>,
        _swdio_tms: Peri<'static, peripherals::PA28>,
        _tdi: Peri<'static, peripherals::PA29>,
        _nreset: Peri<'static, peripherals::PB10>,
        _activity_led: Peri<'static, peripherals::PA10>,
    ) -> Self {
        configure_pin(PIN_TDO, false);
        configure_pin(PIN_SWCLK_TCK, false);
        configure_pin(PIN_SWDIO_TMS, true);
        configure_pin(PIN_TDI, false);
        configure_pin(PAD_NRESET, true);
        configure_pin(PIN_ACTIVITY_LED, false);

        assign_to_fgpio(PORT_A, PIN_TDO);
        assign_to_fgpio(PORT_A, PIN_SWCLK_TCK);
        assign_to_fgpio(PORT_A, PIN_SWDIO_TMS);
        assign_to_fgpio(PORT_A, PIN_TDI);
        assign_to_fgpio(PORT_A, PIN_ACTIVITY_LED);
        assign_to_fgpio(PORT_B, PIN_NRESET);

        let output_a = (SWDIO_TMS | TDI) | activity_led_bit(false);
        let output_b = NRESET;
        write_do(PORT_A, output_a);
        write_do(PORT_B, output_b);
        oe_set(PORT_A, OUTPUT_MASK_A);
        oe_set(PORT_B, OUTPUT_MASK_B);
        oe_clear(PORT_A, TDO);

        Self {
            half_period_delay: swd_delay_for_hz(DEFAULT_SWJ_CLOCK_HZ),
            write_half_period_delay: swd_write_delay_for_hz(DEFAULT_SWJ_CLOCK_HZ),
            swdio_output_enabled: true,
            output_a,
            output_b,
            activity_led_active: false,
            activity_led_lit: false,
            activity_led_next_toggle: 0,
            activity_led_poll_count: 0,
            _owned: PhantomData,
        }
    }

    pub fn set_clock_hz(&mut self, hz: u32) {
        let hz = normalize_swj_clock_hz(hz);
        self.half_period_delay = swd_delay_for_hz(hz);
        self.write_half_period_delay = swd_write_delay_for_hz(hz);
    }

    #[inline(always)]
    pub fn swdio_output(&mut self) {
        if !self.swdio_output_enabled {
            oe_set(PORT_A, SWDIO_TMS);
            self.swdio_output_enabled = true;
        }
    }

    #[inline(always)]
    pub fn swdio_input(&mut self) {
        if self.swdio_output_enabled {
            oe_clear(PORT_A, SWDIO_TMS);
            self.swdio_output_enabled = false;
        }
    }

    #[inline(always)]
    pub fn set_swdio_tms(&mut self, high: bool) {
        self.write_a_level(SWDIO_TMS, high);
    }

    #[inline(always)]
    pub fn set_tdi(&mut self, high: bool) {
        self.write_a_level(TDI, high);
    }

    #[inline(always)]
    pub fn set_swclk_tck(&mut self, high: bool) {
        self.write_a_level(SWCLK_TCK, high);
    }

    #[inline(always)]
    pub fn set_reset(&mut self, high: bool) {
        let output_b = if high {
            self.output_b | NRESET
        } else {
            self.output_b & !NRESET
        };
        if output_b != self.output_b {
            self.output_b = output_b;
            write_do(PORT_B, self.output_b);
        }
    }

    #[inline(always)]
    pub fn activity_led_busy(&mut self) {
        match ACTIVITY_LED_MODE {
            ActivityLedMode::SteadyBusy => self.set_activity_led(true),
            ActivityLedMode::BlinkBusy => self.blink_activity_led(),
        }
    }

    #[inline(always)]
    pub fn activity_led_idle(&mut self) {
        self.activity_led_active = false;
        self.activity_led_poll_count = 0;
        self.set_activity_led(false);
    }

    #[inline(always)]
    pub fn read_swdio(&self) -> bool {
        read_di(PORT_A) & SWDIO_TMS != 0
    }

    #[inline(always)]
    pub fn read_tdo(&self) -> bool {
        read_di(PORT_A) & TDO != 0
    }

    #[inline(always)]
    pub fn swclk_cycle(&mut self) {
        do_clear(PORT_A, SWCLK_TCK);
        self.delay_half();
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        self.delay_half();
    }

    #[inline(always)]
    pub fn swclk_sample_swdio(&mut self) -> bool {
        do_clear(PORT_A, SWCLK_TCK);
        self.delay_half();
        let bit = self.read_swdio();
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        self.delay_half();
        bit
    }

    #[inline(always)]
    pub fn swd_write_bit_cycle(&mut self, high: bool) {
        if high {
            self.output_a = (self.output_a | SWDIO_TMS) & !SWCLK_TCK;
        } else {
            self.output_a &= !(SWDIO_TMS | SWCLK_TCK);
        }
        write_do(PORT_A, self.output_a);
        self.delay_half();
        self.output_a |= SWCLK_TCK;
        write_do(PORT_A, self.output_a);
        self.delay_half();
    }

    #[inline(always)]
    pub fn swd_write_bits(&mut self, mut bits: u32, count: usize) {
        let delay = self.half_period_delay;
        let mut output = self.output_a;

        for _ in 0..count {
            if bits & 1 != 0 {
                output = (output | SWDIO_TMS) & !SWCLK_TCK;
            } else {
                output &= !(SWDIO_TMS | SWCLK_TCK);
            }
            write_do(PORT_A, output);
            delay_half_count(delay);
            output |= SWCLK_TCK;
            do_set(PORT_A, SWCLK_TCK);
            delay_half_count(delay);
            bits >>= 1;
        }

        self.output_a = output;
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swd_write_data_bits(&mut self, mut bits: u32, count: usize) {
        let delay = self.write_half_period_delay;
        if delay == 0 {
            let mut output = self.output_a;

            for _ in 0..count {
                if bits & 1 != 0 {
                    output = (output | SWDIO_TMS) & !SWCLK_TCK;
                } else {
                    output &= !(SWDIO_TMS | SWCLK_TCK);
                }
                write_do(PORT_A, output);
                output |= SWCLK_TCK;
                do_set(PORT_A, SWCLK_TCK);
                bits >>= 1;
            }

            self.output_a = output;
            return;
        }
        if delay != 1 {
            self.swd_write_bits(bits, count);
            return;
        }

        let mut output = self.output_a;

        for _ in 0..count {
            if bits & 1 != 0 {
                output = (output | SWDIO_TMS) & !SWCLK_TCK;
            } else {
                output &= !(SWDIO_TMS | SWCLK_TCK);
            }
            write_do(PORT_A, output);
            delay_half_count(delay);
            output |= SWCLK_TCK;
            do_set(PORT_A, SWCLK_TCK);
            bits >>= 1;
        }

        self.output_a = output;
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swclk_sample_swdio_bits(&mut self, count: usize) -> u32 {
        let delay = self.half_period_delay;
        let mut value = 0;

        for bit in 0..count {
            do_clear(PORT_A, SWCLK_TCK);
            delay_half_count(delay);
            if read_di(PORT_A) & SWDIO_TMS != 0 {
                value |= 1 << bit;
            }
            do_set(PORT_A, SWCLK_TCK);
            delay_half_count(delay);
        }

        self.output_a |= SWCLK_TCK;
        value
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swclk_sample_swdio_3bits(&mut self) -> u8 {
        let delay = self.half_period_delay;
        let mut value = 0;

        do_clear(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
        if read_di(PORT_A) & SWDIO_TMS != 0 {
            value |= 1;
        }
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);

        do_clear(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
        if read_di(PORT_A) & SWDIO_TMS != 0 {
            value |= 2;
        }
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);

        do_clear(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
        if read_di(PORT_A) & SWDIO_TMS != 0 {
            value |= 4;
        }
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);

        self.output_a |= SWCLK_TCK;
        value
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swd_write_block_fast(
        &mut self,
        swd_request: u8,
        data: &[u8],
        count: usize,
        wait_retries: usize,
    ) -> WriteBlockResult {
        self.swdio_output();

        let mut done = 0;
        let mut input = 0;
        while done < count {
            let write_data = u32::from_le_bytes([
                data[input],
                data[input + 1],
                data[input + 2],
                data[input + 3],
            ]);
            let parity = odd_parity(write_data) as u32;
            let mut retry = wait_retries;

            loop {
                let ack = self.swd_write_request_read_ack(swd_request);
                match ack {
                    0b001 => {
                        self.swclk_turnaround_cycle();
                        self.swdio_output();
                        self.swd_write_data_bits(write_data, 32);
                        self.swd_write_data_bits(parity, 1);
                        self.set_swdio_tms(true);
                        break;
                    }
                    0b010 => {
                        self.swclk_turnaround_cycle();
                        self.swdio_output();
                        self.set_swdio_tms(true);
                        if retry == 0 {
                            return WriteBlockResult {
                                status: TransferStatus::Wait,
                                done,
                            };
                        }
                        retry -= 1;
                    }
                    0b100 => {
                        self.swclk_turnaround_cycle();
                        self.swdio_output();
                        self.set_swdio_tms(true);
                        return WriteBlockResult {
                            status: TransferStatus::Fault,
                            done,
                        };
                    }
                    0b111 => {
                        self.finish_protocol_error_fast();
                        return WriteBlockResult {
                            status: TransferStatus::NoAck,
                            done,
                        };
                    }
                    _ => {
                        self.finish_protocol_error_fast();
                        return WriteBlockResult {
                            status: TransferStatus::ProtocolError,
                            done,
                        };
                    }
                }
            }

            input += 4;
            done += 1;
            self.poll_activity_led();
        }

        WriteBlockResult {
            status: TransferStatus::Ok,
            done,
        }
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swd_read_block_fast(
        &mut self,
        swd_request: u8,
        out: &mut [u8],
        count: usize,
        wait_retries: usize,
    ) -> ReadBlockResult {
        self.swdio_output();

        let mut done = 0;
        let mut output = 0;
        while done < count {
            if output + 4 > out.len() {
                return ReadBlockResult {
                    status: TransferStatus::ProtocolError,
                    done,
                };
            }

            let mut retry = wait_retries;
            loop {
                let result = self.swd_read_transfer_fast(swd_request);
                if result.status != TransferStatus::Wait || retry == 0 {
                    if result.status != TransferStatus::Ok {
                        return ReadBlockResult {
                            status: result.status,
                            done,
                        };
                    }
                    out[output..output + 4].copy_from_slice(&result.data.to_le_bytes());
                    break;
                }
                retry -= 1;
            }

            output += 4;
            done += 1;
            self.poll_activity_led();
        }

        ReadBlockResult {
            status: TransferStatus::Ok,
            done,
        }
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swd_write_transfer_fast(&mut self, swd_request: u8, write_data: u32) -> TransferStatus {
        self.poll_activity_led();
        self.swdio_output();
        let ack = self.swd_write_request_read_ack(swd_request);
        match ack {
            0b001 => {
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.swd_write_data_bits(write_data, 32);
                self.swd_write_data_bits(odd_parity(write_data) as u32, 1);
                self.set_swdio_tms(true);
                TransferStatus::Ok
            }
            0b010 => {
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.set_swdio_tms(true);
                TransferStatus::Wait
            }
            0b100 => {
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.set_swdio_tms(true);
                TransferStatus::Fault
            }
            0b111 => {
                self.finish_protocol_error_fast();
                TransferStatus::NoAck
            }
            _ => {
                self.finish_protocol_error_fast();
                TransferStatus::ProtocolError
            }
        }
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    pub fn swd_read_transfer_fast(&mut self, swd_request: u8) -> crate::swj::TransferResult {
        self.poll_activity_led();
        self.swdio_output();
        let ack = self.swd_write_request_read_ack(swd_request);
        match ack {
            0b001 => {
                let data = self.swclk_sample_swdio_bits(32);
                let parity = self.swclk_sample_swdio();
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.set_swdio_tms(true);
                if parity == odd_parity(data) {
                    crate::swj::TransferResult {
                        status: TransferStatus::Ok,
                        data,
                    }
                } else {
                    crate::swj::TransferResult {
                        status: TransferStatus::ParityError,
                        data,
                    }
                }
            }
            0b010 => {
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.set_swdio_tms(true);
                crate::swj::TransferResult {
                    status: TransferStatus::Wait,
                    data: 0,
                }
            }
            0b100 => {
                self.swclk_turnaround_cycle();
                self.swdio_output();
                self.set_swdio_tms(true);
                crate::swj::TransferResult {
                    status: TransferStatus::Fault,
                    data: 0,
                }
            }
            0b111 => {
                self.finish_protocol_error_fast();
                crate::swj::TransferResult {
                    status: TransferStatus::NoAck,
                    data: 0,
                }
            }
            _ => {
                self.finish_protocol_error_fast();
                crate::swj::TransferResult {
                    status: TransferStatus::ProtocolError,
                    data: 0,
                }
            }
        }
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    fn swd_write_request_read_ack(&mut self, swd_request: u8) -> u8 {
        self.swd_write_data_bits(swd_request as u32, 8);
        self.swdio_input();
        self.swclk_turnaround_cycle();
        self.swclk_sample_swdio_3bits()
    }

    #[inline(always)]
    #[unsafe(link_section = ".fast")]
    fn finish_protocol_error_fast(&mut self) {
        self.swdio_input();
        let _ = self.swclk_sample_swdio_bits(34);
        self.swdio_output();
        self.set_swdio_tms(true);
    }

    #[inline(always)]
    pub fn tck_cycle(&mut self) {
        self.swclk_cycle();
    }

    #[inline(always)]
    pub fn jtag_cycle_tdi(&mut self, high: bool) {
        let delay = self.write_half_period_delay;
        if high {
            self.output_a = (self.output_a | TDI) & !SWCLK_TCK;
        } else {
            self.output_a &= !(TDI | SWCLK_TCK);
        }
        write_do(PORT_A, self.output_a);
        delay_half_count(delay);
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
    }

    #[inline(always)]
    pub fn jtag_cycle_tdio(&mut self, high: bool) -> bool {
        let delay = self.half_period_delay;
        if high {
            self.output_a = (self.output_a | TDI) & !SWCLK_TCK;
        } else {
            self.output_a &= !(TDI | SWCLK_TCK);
        }
        write_do(PORT_A, self.output_a);
        delay_half_count(delay);
        let bit = read_di(PORT_A) & TDO != 0;
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
        bit
    }

    #[inline(always)]
    pub fn tck_sample_tdo(&mut self) -> bool {
        do_clear(PORT_A, SWCLK_TCK);
        self.delay_half();
        let bit = self.read_tdo();
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        self.delay_half();
        bit
    }

    pub fn current_pin_state(&self) -> u8 {
        let di_a = read_di(PORT_A);
        let di_b = read_di(PORT_B);
        let mut pins = 0;
        if di_a & SWCLK_TCK != 0 {
            pins |= 0x01;
        }
        if di_a & SWDIO_TMS != 0 {
            pins |= 0x02;
        }
        if di_a & TDI != 0 {
            pins |= 0x04;
        }
        if di_a & TDO != 0 {
            pins |= 0x08;
        }
        pins |= 0x20;
        if di_b & NRESET != 0 {
            pins |= 0x80;
        }
        pins
    }

    #[inline(always)]
    fn write_a_level(&mut self, mask: u32, high: bool) {
        let output_a = if high {
            self.output_a | mask
        } else {
            self.output_a & !mask
        };
        if output_a != self.output_a {
            self.output_a = output_a;
            write_do(PORT_A, self.output_a);
        }
    }

    #[inline(always)]
    fn set_activity_led(&mut self, lit: bool) {
        if self.activity_led_lit == lit {
            return;
        }
        self.activity_led_lit = lit;
        let output_a = (self.output_a & !ACTIVITY_LED) | activity_led_bit(lit);
        self.output_a = output_a;
        write_do(PORT_A, output_a);
    }

    #[inline(always)]
    fn blink_activity_led(&mut self) {
        let now = time::now_ticks();
        if !self.activity_led_active {
            self.activity_led_active = true;
            self.activity_led_next_toggle =
                now.wrapping_add(time::ticks_from_millis(ACTIVITY_LED_BLINK_TOGGLE_MS));
            self.set_activity_led(true);
            return;
        }

        if (now.wrapping_sub(self.activity_led_next_toggle) as i64) >= 0 {
            self.activity_led_next_toggle =
                now.wrapping_add(time::ticks_from_millis(ACTIVITY_LED_BLINK_TOGGLE_MS));
            self.set_activity_led(!self.activity_led_lit);
        }
    }

    #[inline(always)]
    fn poll_activity_led(&mut self) {
        if self.activity_led_poll_count != 0 {
            self.activity_led_poll_count -= 1;
            return;
        }

        self.activity_led_poll_count = ACTIVITY_LED_POLL_INTERVAL;
        self.activity_led_busy();
    }

    #[inline(always)]
    fn delay_half(&self) {
        delay_half_count(self.half_period_delay);
    }

    #[inline(always)]
    fn swclk_turnaround_cycle(&mut self) {
        let delay = self.write_half_period_delay;
        do_clear(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
        self.output_a |= SWCLK_TCK;
        do_set(PORT_A, SWCLK_TCK);
        delay_half_count(delay);
    }
}

impl ProbeIo for ProbePins {
    #[inline(always)]
    fn set_clock_hz(&mut self, hz: u32) {
        ProbePins::set_clock_hz(self, hz);
    }

    #[inline(always)]
    fn swdio_output(&mut self) {
        ProbePins::swdio_output(self);
    }

    #[inline(always)]
    fn swdio_input(&mut self) {
        ProbePins::swdio_input(self);
    }

    #[inline(always)]
    fn set_swdio_tms(&mut self, high: bool) {
        ProbePins::set_swdio_tms(self, high);
    }

    #[inline(always)]
    fn set_tdi(&mut self, high: bool) {
        ProbePins::set_tdi(self, high);
    }

    #[inline(always)]
    fn set_swclk_tck(&mut self, high: bool) {
        ProbePins::set_swclk_tck(self, high);
    }

    #[inline(always)]
    fn set_reset(&mut self, high: bool) {
        ProbePins::set_reset(self, high);
    }

    #[inline(always)]
    fn swclk_cycle(&mut self) {
        ProbePins::swclk_cycle(self);
    }

    #[inline(always)]
    fn swclk_sample_swdio(&mut self) -> bool {
        ProbePins::swclk_sample_swdio(self)
    }

    #[inline(always)]
    fn swd_write_bit_cycle(&mut self, high: bool) {
        ProbePins::swd_write_bit_cycle(self, high);
    }

    #[inline(always)]
    fn swd_write_bits(&mut self, bits: u32, count: usize) {
        ProbePins::swd_write_bits(self, bits, count);
    }

    #[inline(always)]
    fn swd_write_data_bits(&mut self, bits: u32, count: usize) {
        ProbePins::swd_write_data_bits(self, bits, count);
    }

    #[inline(always)]
    fn swclk_sample_swdio_bits(&mut self, count: usize) -> u32 {
        ProbePins::swclk_sample_swdio_bits(self, count)
    }

    #[inline(always)]
    fn tck_cycle(&mut self) {
        ProbePins::tck_cycle(self);
    }

    #[inline(always)]
    fn tck_sample_tdo(&mut self) -> bool {
        ProbePins::tck_sample_tdo(self)
    }

    #[inline(always)]
    fn jtag_cycle_tdi(&mut self, high: bool) -> bool {
        ProbePins::jtag_cycle_tdi(self, high);
        true
    }

    #[inline(always)]
    fn jtag_cycle_tdio(&mut self, high: bool) -> Option<bool> {
        Some(ProbePins::jtag_cycle_tdio(self, high))
    }

    #[inline(always)]
    fn current_pin_state(&self) -> u8 {
        ProbePins::current_pin_state(self)
    }

    #[inline(always)]
    fn activity_led_busy(&mut self) {
        ProbePins::activity_led_busy(self);
    }

    #[inline(always)]
    fn activity_led_idle(&mut self) {
        ProbePins::activity_led_idle(self);
    }

    #[inline(always)]
    fn swd_write_block(
        &mut self,
        request: u8,
        data: &[u8],
        count: usize,
        wait_retries: usize,
        turnaround: u8,
        idle_cycles: u8,
        data_phase_on_wait_fault: bool,
    ) -> Option<WriteBlockResult> {
        if turnaround == 1 && idle_cycles == 0 && !data_phase_on_wait_fault {
            Some(ProbePins::swd_write_block_fast(
                self,
                make_swd_request(request),
                data,
                count,
                wait_retries,
            ))
        } else {
            None
        }
    }

    #[inline(always)]
    fn swd_read_block(
        &mut self,
        request: u8,
        out: &mut [u8],
        count: usize,
        wait_retries: usize,
        turnaround: u8,
        idle_cycles: u8,
        data_phase_on_wait_fault: bool,
    ) -> Option<ReadBlockResult> {
        if turnaround == 1 && idle_cycles == 0 && !data_phase_on_wait_fault {
            Some(ProbePins::swd_read_block_fast(
                self,
                make_swd_request(request),
                out,
                count,
                wait_retries,
            ))
        } else {
            None
        }
    }

    #[inline(always)]
    fn swd_transfer(
        &mut self,
        request: u8,
        write_data: u32,
        turnaround: u8,
        idle_cycles: u8,
        data_phase_on_wait_fault: bool,
    ) -> Option<crate::swj::TransferResult> {
        if request & 0x02 == 0 && turnaround == 1 && idle_cycles == 0 && !data_phase_on_wait_fault {
            Some(crate::swj::TransferResult {
                status: ProbePins::swd_write_transfer_fast(
                    self,
                    make_swd_request(request),
                    write_data,
                ),
                data: 0,
            })
        } else if request & 0x02 != 0
            && turnaround == 1
            && idle_cycles == 0
            && !data_phase_on_wait_fault
        {
            Some(ProbePins::swd_read_transfer_fast(
                self,
                make_swd_request(request),
            ))
        } else {
            None
        }
    }

    #[inline(always)]
    fn swd_transfer_precomputed(
        &mut self,
        swd_request: u8,
        write_data: u32,
        turnaround: u8,
        idle_cycles: u8,
        data_phase_on_wait_fault: bool,
    ) -> Option<crate::swj::TransferResult> {
        if swd_request & 0x04 == 0
            && turnaround == 1
            && idle_cycles == 0
            && !data_phase_on_wait_fault
        {
            Some(crate::swj::TransferResult {
                status: ProbePins::swd_write_transfer_fast(self, swd_request, write_data),
                data: 0,
            })
        } else if swd_request & 0x04 != 0
            && turnaround == 1
            && idle_cycles == 0
            && !data_phase_on_wait_fault
        {
            Some(ProbePins::swd_read_transfer_fast(self, swd_request))
        } else {
            None
        }
    }
}

#[inline(always)]
fn delay_half_count(count: u8) {
    match count {
        0 => {}
        1 => core::hint::spin_loop(),
        2 => {
            core::hint::spin_loop();
            core::hint::spin_loop();
        }
        3 => {
            core::hint::spin_loop();
            core::hint::spin_loop();
            core::hint::spin_loop();
        }
        5 => {
            core::hint::spin_loop();
            core::hint::spin_loop();
            core::hint::spin_loop();
            core::hint::spin_loop();
            core::hint::spin_loop();
        }
        n => {
            for _ in 0..n {
                core::hint::spin_loop();
            }
        }
    }
}

fn swd_delay_for_hz(hz: u32) -> u8 {
    match hz {
        0..=250_000 => 96,
        250_001..=500_000 => 48,
        500_001..=1_000_000 => 24,
        1_000_001..=2_000_000 => 10,
        2_000_001..=3_000_000 => 7,
        3_000_001..=4_000_000 => 5,
        4_000_001..=8_000_000 => 3,
        8_000_001..=20_000_000 => 1,
        _ => 0,
    }
}

fn swd_write_delay_for_hz(hz: u32) -> u8 {
    swd_delay_for_hz(hz)
}

fn normalize_swj_clock_hz(hz: u32) -> u32 {
    if hz == PROBE_RS_DEFAULT_SWJ_CLOCK_HZ {
        DEFAULT_SWJ_CLOCK_HZ
    } else {
        hz.min(MAX_STABLE_SWJ_CLOCK_HZ)
    }
}

const fn activity_led_bit(lit: bool) -> u32 {
    if lit == ACTIVITY_LED_ACTIVE_HIGH {
        ACTIVITY_LED
    } else {
        0
    }
}

#[inline(always)]
fn make_swd_request(dap_request: u8) -> u8 {
    let ap = dap_request & 0x01 != 0;
    let read = dap_request & 0x02 != 0;
    let a2 = dap_request & 0x04 != 0;
    let a3 = dap_request & 0x08 != 0;
    let parity = (ap as u8 ^ read as u8 ^ a2 as u8 ^ a3 as u8) & 1 != 0;

    1 | ((ap as u8) << 1)
        | ((read as u8) << 2)
        | ((a2 as u8) << 3)
        | ((a3 as u8) << 4)
        | ((parity as u8) << 5)
        | (1 << 7)
}

#[inline(always)]
fn odd_parity(value: u32) -> bool {
    let mut bits = value;
    bits ^= bits >> 16;
    bits ^= bits >> 8;
    bits ^= bits >> 4;
    (0x6996 >> (bits & 0x0f)) & 1 != 0
}

fn configure_pin(number: u8, pull_up: bool) {
    let ioc = unsafe { &*pac::Ioc::ptr() };
    let pad = ioc.pad(number as usize);

    unsafe {
        pad.func_ctl().write(|w| {
            w.alt_select()
                .bits(0)
                .analog()
                .clear_bit()
                .loop_back()
                .clear_bit()
        });

        pad.pad_ctl().write(|w| {
            w.ds()
                .bits(7)
                .spd()
                .bits(3)
                .sr()
                .set_bit()
                .od()
                .clear_bit()
                .ke()
                .clear_bit()
                .pe()
                .bit(pull_up)
                .ps()
                .bit(pull_up)
                .prs()
                .bits(0)
                .hys()
                .bit(pull_up)
        });
    }
}

fn assign_to_fgpio(port: usize, number: u8) {
    let gpiom = unsafe { &*pac::Gpiom::ptr() };
    unsafe {
        gpiom
            .assign(port)
            .pin(number as usize)
            .write(|w| w.select().bits(2).hide().bits(1).lock().clear_bit());
    }
}

#[inline(always)]
fn read_di(port: usize) -> u32 {
    unsafe { ptr::read_volatile((FGPIO_BASE + port * 16) as *const u32) }
}

#[inline(always)]
fn write_do(port: usize, value: u32) {
    unsafe { ptr::write_volatile((FGPIO_BASE + DO_BASE + port * 16) as *mut u32, value) };
}

#[inline(always)]
fn do_set(port: usize, mask: u32) {
    unsafe { ptr::write_volatile((FGPIO_BASE + DO_SET_OFFSET + port * 16) as *mut u32, mask) };
}

#[inline(always)]
fn do_clear(port: usize, mask: u32) {
    unsafe { ptr::write_volatile((FGPIO_BASE + DO_CLR_OFFSET + port * 16) as *mut u32, mask) };
}

#[inline(always)]
fn oe_set(port: usize, mask: u32) {
    unsafe { ptr::write_volatile((FGPIO_BASE + OE_BASE + port * 16 + 4) as *mut u32, mask) };
}

#[inline(always)]
fn oe_clear(port: usize, mask: u32) {
    unsafe { ptr::write_volatile((FGPIO_BASE + OE_BASE + port * 16 + 8) as *mut u32, mask) };
}
