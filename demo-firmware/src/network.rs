use defmt::info;
use embassy_embedded_hal::shared_bus::asynch::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_futures::yield_now;
use embassy_net::{Config, Stack, StackResources, StaticConfigV4};
use embassy_net_wiznet::{Device, Runner, State, chip::W5500};
use embassy_rp::{
    clocks::RoscRng,
    gpio::{Input, Level, Output, Pull},
    peripherals::{SPI0, SPI1},
    spi::Spi,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use static_cell::StaticCell;

fn w5500_spi_config() -> embassy_rp::spi::Config {
    let mut spi_config = embassy_rp::spi::Config::default();
    spi_config.frequency = 50_000_000;
    spi_config.phase = embassy_rp::spi::Phase::CaptureOnSecondTransition;
    spi_config.polarity = embassy_rp::spi::Polarity::IdleHigh;
    spi_config
}

pub(crate) static ETHERNET_1_CONFIG: Mutex<CriticalSectionRawMutex, Option<StaticConfigV4>> =
    Mutex::new(None);

pub(crate) async fn init_ethernet_1(
    r: crate::Ethernet1Resources,
    spawner: Spawner,
) -> Stack<'static> {
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
    let _ = ETHERNET_1_CONFIG.lock().await.replace(cfg.clone());
    let local_addr = cfg.address.address();
    info!("IP address: {:?}", local_addr);

    stack
}

pub(crate) static ETHERNET_2_CONFIG: Mutex<CriticalSectionRawMutex, Option<StaticConfigV4>> =
    Mutex::new(None);

pub(crate) async fn init_ethernet_2(
    r: crate::Ethernet2Resources,
    spawner: Spawner,
) -> Stack<'static> {
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
    let _ = ETHERNET_2_CONFIG.lock().await.replace(cfg.clone());
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
