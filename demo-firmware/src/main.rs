#![no_std]
#![no_main]

use defmt::{info, warn};
use defmt_rtt as _;
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_net::{Config, Stack, StackResources};
use embassy_net_wiznet::{Device, Runner, State, chip::W5500};
use embassy_rp::{
    Peri, bind_interrupts,
    clocks::RoscRng,
    gpio::{Input, Level, Output, Pull},
    peripherals::{self, I2C1, SPI0, SPI1},
    spi::Spi,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use heapless::Vec;
use panic_probe as _;
use portable_atomic as _;
use static_cell::StaticCell;

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
    i2c: I2cResources {
        peripheral: I2C1,
        sda: PIN_14,
        scl: PIN_15,
    },
    buttons: ButtonResources {
        user_1: PIN_22,
        user_2: PIN_26,
        user_3: PIN_28,
    },
}

bind_interrupts!(struct Irqs {
    I2C1_IRQ => embassy_rp::i2c::InterruptHandler<I2C1>;
});

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let r = split_resources!(p);

    info!("Hello, world!");

    let _i2c = embassy_rp::i2c::I2c::new_async(
        r.i2c.peripheral,
        r.i2c.scl,
        r.i2c.sda,
        Irqs,
        Default::default(),
    );

    spawner.must_spawn(button_task(r.buttons));

    let net_stack_1 = init_ethernet_1(r.ethernet_1, spawner).await;
    let net_stack_2 = init_ethernet_2(r.ethernet_2, spawner).await;

    spawner.must_spawn(listen_task(net_stack_1, 0, 1234));
    spawner.must_spawn(listen_task(net_stack_1, 1, 1234));

    spawner.must_spawn(repeat_task(net_stack_2, 1234));
}

#[embassy_executor::task]
async fn button_task(r: ButtonResources) -> ! {
    let btn_1 = Input::new(r.user_1, Pull::Up);
    let btn_2 = Input::new(r.user_2, Pull::Up);
    let btn_3 = Input::new(r.user_3, Pull::Up);

    loop {
        info!(
            "Button 1/2/3: {}/{}/{}",
            btn_1.get_level(),
            btn_2.get_level(),
            btn_3.get_level(),
        );

        Timer::after_secs(1).await;
    }
}

fn w5500_spi_config() -> embassy_rp::spi::Config {
    let mut spi_config = embassy_rp::spi::Config::default();
    spi_config.frequency = 50_000_000;
    spi_config.phase = embassy_rp::spi::Phase::CaptureOnSecondTransition;
    spi_config.polarity = embassy_rp::spi::Polarity::IdleHigh;
    spi_config
}

async fn init_ethernet_1(r: crate::Ethernet1Resources, spawner: Spawner) -> Stack<'static> {
    let spi = Spi::new(
        r.spi,
        r.clk,
        r.mosi,
        r.miso,
        r.tx_dma,
        r.rx_dma,
        w5500_spi_config(),
    );

    static SPI: StaticCell<
        Mutex<CriticalSectionRawMutex, Spi<'static, SPI0, embassy_rp::spi::Async>>,
    > = StaticCell::new();
    let spi = SPI.init(Mutex::new(spi));

    let cs = Output::new(r.cs_pin, Level::High);
    let device = SpiDeviceWithConfig::new(spi, cs, w5500_spi_config());

    let w5500_int = Input::new(r.int_pin, Pull::Up);
    let w5500_reset = Output::new(r.rst_pin, Level::High);

    let mac_addr = [0x02, 0x00, 0x00, 0x00, 0x55, 0x01];

    static STATE: StaticCell<State<8, 8>> = StaticCell::new();
    let state = STATE.init(State::<8, 8>::new());

    let (device, runner) = embassy_net_wiznet::new(mac_addr, state, device, w5500_int, w5500_reset)
        .await
        .unwrap();

    spawner.must_spawn(ethernet_1_task(runner));

    static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();
    let mut rng = RoscRng;
    let (stack, runner) = embassy_net::new(
        device,
        Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        rng.next_u64(),
    );
    spawner.must_spawn(net_task(runner));

    info!("Waiting for DHCP...");
    let cfg = wait_for_config(stack).await;
    let local_addr = cfg.address.address();
    info!("IP address: {:?}", local_addr);

    stack
}

async fn init_ethernet_2(r: crate::Ethernet2Resources, spawner: Spawner) -> Stack<'static> {
    let spi = Spi::new(
        r.spi,
        r.clk,
        r.mosi,
        r.miso,
        r.tx_dma,
        r.rx_dma,
        w5500_spi_config(),
    );

    static SPI: StaticCell<
        Mutex<CriticalSectionRawMutex, Spi<'static, SPI1, embassy_rp::spi::Async>>,
    > = StaticCell::new();
    let spi = SPI.init(Mutex::new(spi));

    let cs = Output::new(r.cs_pin, Level::High);
    let device = SpiDeviceWithConfig::new(spi, cs, w5500_spi_config());

    let w5500_int = Input::new(r.int_pin, Pull::Up);
    let w5500_reset = Output::new(r.rst_pin, Level::High);

    let mac_addr = [0x02, 0x00, 0x00, 0x00, 0x55, 0x02];

    static STATE: StaticCell<State<8, 8>> = StaticCell::new();
    let state = STATE.init(State::<8, 8>::new());

    let (device, runner) = embassy_net_wiznet::new(mac_addr, state, device, w5500_int, w5500_reset)
        .await
        .unwrap();

    spawner.must_spawn(ethernet_2_task(runner));

    static RESOURCES: StaticCell<StackResources<8>> = StaticCell::new();
    let mut rng = RoscRng;
    let (stack, runner) = embassy_net::new(
        device,
        Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        rng.next_u64(),
    );
    spawner.must_spawn(net_task(runner));

    info!("Waiting for DHCP...");
    let cfg = wait_for_config(stack).await;
    let local_addr = cfg.address.address();
    info!("IP address: {:?}", local_addr);

    stack
}

type EthernetSpi<SPI> = SpiDeviceWithConfig<
    'static,
    CriticalSectionRawMutex,
    Spi<'static, SPI, embassy_rp::spi::Async>,
    Output<'static>,
>;

#[embassy_executor::task]
async fn ethernet_1_task(
    runner: Runner<'static, W5500, EthernetSpi<SPI0>, Input<'static>, Output<'static>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn ethernet_2_task(
    runner: Runner<'static, W5500, EthernetSpi<SPI1>, Input<'static>, Output<'static>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task(pool_size = 2)]
async fn net_task(mut runner: embassy_net::Runner<'static, Device<'static>>) -> ! {
    runner.run().await
}

async fn wait_for_config(stack: Stack<'static>) -> embassy_net::StaticConfigV4 {
    loop {
        if let Some(config) = stack.config_v4() {
            return config.clone();
        }
        yield_now().await;
    }
}

static CHANNEL: Channel<CriticalSectionRawMutex, Vec<u8, 256>, 8> = Channel::new();

#[embassy_executor::task(pool_size = 2)]
async fn listen_task(stack: Stack<'static>, id: u8, port: u16) {
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
async fn repeat_task(stack: Stack<'static>, port: u16) {
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
