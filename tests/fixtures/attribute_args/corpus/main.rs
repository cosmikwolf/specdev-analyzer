//! hello-rtic — generated from specs/modules/Ticker.toml and
//! specs/modules/Blinky.toml (firmware-layered + rtic + mk20dx).
//!
//! One `#[rtic::app]` per crate (rtic prose §1), so both spec modules'
//! `[rtic]` tables land in this single app. The `device` path and the
//! dispatcher vector are chip facts from the mk20dx prose §4.3.1:
//! `mk20dx_hal` completes the svd2rust device contract, and the CAN0
//! vectors are free on a CAN-less design. Both software tasks run at
//! priority 1, so one dispatcher suffices.
// @spec specs/modules/Ticker.toml
// @spec specs/modules/Blinky.toml
#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

/// The PAC's `rt` feature installs the full 92-entry vector table, and
/// every unpopulated entry needs somewhere to land (mk20dx prose
/// §4.3.1): without this, the link fails with `symbol not found:
/// DefaultHandler_`.
#[cortex_m_rt::exception]
unsafe fn DefaultHandler(irqn: i16) {
    panic!("unhandled interrupt {}", irqn);
}

#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    use embedded_hal::digital::StatefulOutputPin;
    use fugit::ExtU32;
    use mk20dx_hal::clocks::{ClockSpeed, McgExt};
    use mk20dx_hal::gpio::{GpioExt, Output, Pin, PushPull};
    use mk20dx_hal::timer::{PitChannel, PitExt};

    /// `[[rtic.resources]]` sharing = "shared" (Ticker).
    #[shared]
    struct Shared {
        count: u32,
    }

    /// `[[rtic.resources]]` sharing = "local" (Blinky's `led_on`), plus
    /// the hardware handles each task owns.
    #[local]
    struct Local {
        led_on: bool,
        tick: PitChannel<0>,
        status_led: Pin<'C', 5, Output<PushPull>>,
    }

    #[init]
    fn init(cx: init::Context) -> (Shared, Local) {
        let dp = unsafe { mk20dx_hal::pac::Peripherals::steal() };
        let _ = cx;

        // [mk20dx.clock] speed = "mhz72" (Ticker owns the preset).
        let clocks = dp.mcg.constrain().freeze_at(ClockSpeed::Mhz72, dp.osc, &dp.sim);

        // Port split — once, in init (mk20dx prose §2).
        let pins_c = dp.portc.split(dp.ptc, &dp.sim);

        // [[mk20dx.pins]] status_led, C5, push_pull_output (Blinky).
        let status_led = pins_c.pc5.into_push_pull_output();

        // [[mk20dx.timers]] tick, channel 0, period_us = 1000 — with the
        // PIT0 vector declared, so the interrupt is enabled (prose §6).
        let pit = dp.pit.split(&dp.sim, &clocks);
        let mut tick: PitChannel<0> = pit.ch0;
        tick.start(1000u32.micros());
        tick.enable_interrupt();

        drain::spawn().ok();
        blink::spawn().ok();

        (
            Shared { count: 0 },
            Local { led_on: false, tick, status_led },
        )
    }

    /// Ticker `[[rtic.tasks]]` on_tick: hardware, binds PIT0, priority 3.
    /// Clears the timer flag first (mk20dx prose §4.1 — a handler that
    /// returns without clearing re-enters forever), then bumps the
    /// shared counter through the lock.
    #[task(binds = PIT0, shared = [count], local = [tick], priority = 3)]
    fn on_tick(mut cx: on_tick::Context) {
        cx.local.tick.clear_interrupt();
        cx.shared.count.lock(|count: &mut u32| *count += 1);
    }

    /// Ticker `[[rtic.tasks]]` drain: software, priority 1.
    #[task(shared = [count], priority = 1)]
    async fn drain(mut cx: drain::Context) {
        let seen = cx.shared.count.lock(|count: &mut u32| {
            let v = *count;
            *count = 0;
            v
        });
        defmt::info!("drained {} ticks", seen);
    }

    /// Blinky `[[rtic.tasks]]` blink: software, priority 1.
    #[task(local = [led_on, status_led], priority = 1)]
    async fn blink(cx: blink::Context) {
        *cx.local.led_on = !*cx.local.led_on;
        cx.local.status_led.toggle().ok();
    }

    /// Runs when nothing else is runnable.
    #[idle]
    fn idle(_cx: idle::Context) -> ! {
        loop {
            cortex_m::asm::wfi();
        }
    }
}
