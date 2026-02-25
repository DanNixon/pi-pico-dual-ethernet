#![no_std]
#![no_main]

mod buttons;
mod display;
mod network;
mod wall_time;

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_rp::{Peri, peripherals};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, pubsub::WaitResult,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use heapless::Vec;
use panic_probe as _;
use portable_atomic as _;

assign_resources::assign_resources! {
    // Ethernet 1 is the same pins as the WIZnet evaluation boards
    ethernet_1: Ethernet1Resources {
        miso: PIN_16,
        mosi: PIN_19,
        clk: PIN_18,
        spi: SPI0,
        tx_dma: DMA_CH0,
        rx_dma: DMA_CH1,
        cs_pin: PIN_17,
        int_pin: PIN_21,
        rst_pin: PIN_20,
    },
    ethernet_2: Ethernet2Resources {
        miso: PIN_8,
        mosi: PIN_11,
        clk: PIN_10,
        spi: SPI1,
        tx_dma: DMA_CH2,
        rx_dma: DMA_CH3,
        cs_pin: PIN_9,
        int_pin: PIN_13,
        rst_pin: PIN_12,
    },
    display: DisplayResources {
        i2c: I2C1,
        sda: PIN_14,
        scl: PIN_15,
    },
    buttons: ButtonResources {
        user_1: PIN_22,
        user_2: PIN_26,
        user_3: PIN_28,
    },
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let r = split_resources!(p);

    spawner.must_spawn(display::task(r.display));

    spawner.must_spawn(say_button_task());
    buttons::init(r.buttons, spawner);

    let net_stack_1 = network::init_ethernet_1(r.ethernet_1, spawner).await;
    let net_stack_2 = network::init_ethernet_2(r.ethernet_2, spawner).await;

    spawner.must_spawn(say_time_task());
    spawner.must_spawn(wall_time::ntp_task(net_stack_2));

    spawner.must_spawn(listen_task(net_stack_1, 0, 1234));
    spawner.must_spawn(listen_task(net_stack_1, 1, 1234));

    spawner.must_spawn(repeat_task(net_stack_2, 1234));
}

#[embassy_executor::task]
async fn say_button_task() -> ! {
    let mut subscriber = buttons::BUTTON_EVENTS.subscriber().unwrap();

    loop {
        match subscriber.next_message().await {
            WaitResult::Lagged(_) => {
                unreachable!();
            }
            WaitResult::Message(event) => {
                info!("Button: {}", event);
            }
        }
    }
}

#[embassy_executor::task]
async fn say_time_task() -> ! {
    loop {
        info!("Wall clock: {}", wall_time::now());
        Timer::after_secs(1).await;
    }
}

static CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8, 256>, 8> = Channel::new();

#[embassy_executor::task(pool_size = 2)]
async fn listen_task(stack: Stack<'static>, id: u8, port: u16) -> ! {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];
    let mut buf = [0; 4096];
    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("SOCKET {}: Listening on TCP:{}...", id, port);
        if let Err(e) = socket.accept(port).await {
            warn!("accept error: {:?}", e);
            continue;
        }
        info!(
            "SOCKET {}: Received connection from {:?}",
            id,
            socket.remote_endpoint()
        );

        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) => {
                    warn!("read EOF");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("SOCKET {}: {:?}", id, e);
                    break;
                }
            };
            info!(
                "SOCKET {}: rxd {}",
                id,
                core::str::from_utf8(&buf[..n]).unwrap()
            );

            let vec = Vec::try_from(&buf[..n]).unwrap();
            if CHANNEL.try_send(vec).is_err() {
                warn!("Failed to send on channel");
            }
        }
    }
}

#[embassy_executor::task]
async fn repeat_task(stack: Stack<'static>, port: u16) -> ! {
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        let mut socket = embassy_net::tcp::TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("SOCKET: Listening on TCP:{}...", port);
        if let Err(e) = socket.accept(port).await {
            warn!("accept error: {:?}", e);
            continue;
        }
        info!(
            "SOCKET: Received connection from {:?}",
            socket.remote_endpoint()
        );

        loop {
            let data = CHANNEL.receive().await;

            if let Err(e) = socket.write_all(&data).await {
                warn!("write error: {:?}", e);
                break;
            }
        }
    }
}
