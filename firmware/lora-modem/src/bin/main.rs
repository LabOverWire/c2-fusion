#![no_std]
#![no_main]
#![deny(clippy::mem_forget)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;
use log::{error, info};

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
async fn heartbeat(period: Duration) {
    loop {
        info!("lora-modem alive");
        Timer::after(period).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0, peripherals.FROM_CPU_INTR0);

    info!("lora-modem started");
    match heartbeat(Duration::from_secs(1)) {
        Ok(token) => spawner.spawn(token),
        Err(error) => error!("heartbeat spawn failed: {error:?}"),
    }

    loop {
        Timer::after(Duration::from_secs(3600)).await;
    }
}
