pub mod io;

use drivers::ps2::io::{ControllerCommand, ControllerReturnCommand, DeviceCommand, Ps2Error};
use spin::Mutex;

pub const RESEND: u8 = 0xFE;
pub const ACK: u8 = 0xFA;

lazy_static! {
    pub static ref PS2: Mutex<Controller> = Mutex::new(Controller::new());
}

bitflags! {
    pub struct ConfigFlags: u8 {
        /// If interrupts for Port 1 are enabled
        const PORT_INTERRUPT_1 = 1 << 0;
        /// If interrupts for Port 2 are enabled
        const PORT_INTERRUPT_2 = 1 << 1;
        /// If the clock for Port 1 is disabled
        const PORT_CLOCK_1 = 1 << 4;
        /// If the clock for Port 2 is disabled
        const PORT_CLOCK_2 = 1 << 5;
        /// If the controller will transform scan set 2 to scan set 1
        const PORT_TRANSLATION_1 = 1 << 6;
    }
}

/// Represents the PS2 master controller
pub struct Controller {
    pub devices: (Device, Device),
    config: ConfigFlags,
}

impl Controller {
    fn new() -> Self {
        Controller {
            devices: (
                Device::new(DevicePort::Keyboard),
                Device::new(DevicePort::Mouse),
            ),
            config: ConfigFlags::empty(),
        }
    }

    /// Initializes this PS2 controller
    pub fn initialize(&mut self) -> Result<(), Ps2Error> {
        println!("ps2c: initializing");

        self.prepare_devices()?;
        println!("ps2c: disabled devices");

        io::flush_output()?;

        self.initialize_config()?;

        if !self.test_controller()? {
            println!("ps2c: controller test failed");
        }

        println!("ps2c: testing devices");
        match self.test_devices()? {
            (false, _) => println!("ps2c: first device not supported"),
            (_, false) => println!("ps2c: second device not supported"),
            _ => (),
        }

        // Check if any devices are available
        if self.reset_devices()? > 0 {
            println!("ps2c: prepared devices");
        } else {
            println!("ps2c: detected no available devices");
        }

        io::flush_output()?;

        Ok(())
    }

    /// Writes the given config to the PS2 controller
    pub fn write_config(&self, config: ConfigFlags) -> Result<(), Ps2Error> {
        io::command_data(ControllerCommand::WriteConfig, config.bits())
    }

    /// Reads the config from the PS2 controller
    pub fn read_config(&self) -> Result<ConfigFlags, Ps2Error> {
        let read = io::command_ret(ControllerReturnCommand::ReadConfig)?;

        Ok(ConfigFlags::from_bits_truncate(read))
    }

    /// Resets this controller's devices and prepares them for initialization
    fn prepare_devices(&mut self) -> Result<(), Ps2Error> {
        self.devices.0.disable()?;
        self.devices.1.disable()?;

        self.devices.0.state = DeviceState::Unavailable;
        self.devices.1.state = DeviceState::Unavailable;

        Ok(())
    }

    /// Initializes the config for this controller
    fn initialize_config(&mut self) -> Result<(), Ps2Error> {
        // Read the config from the controller
        self.config = self.read_config()?;

        // Set all required config flags
        self.config.set(ConfigFlags::PORT_INTERRUPT_1, false);
        self.config.set(ConfigFlags::PORT_INTERRUPT_2, false);
        self.config.set(ConfigFlags::PORT_TRANSLATION_1, false);

        // Write the updated config back to the controller
        self.write_config(self.config)?;

        println!("ps2c: initialized config");

        Ok(())
    }

    /// Tests this controller
    fn test_controller(&self) -> Result<bool, Ps2Error> {
        Ok(io::command_ret(ControllerReturnCommand::TestController)? == 0x55)
    }

    /// Tests all of this controller's devices
    fn test_devices(&mut self) -> Result<(bool, bool), Ps2Error> {
        // Check if controller supports the second device
        if self.config.contains(ConfigFlags::PORT_CLOCK_2) {
            self.devices.1.enable()?;
            self.config = self.read_config()?;
            self.devices.1.disable()?;
        }

        // Test both devices
        let first_supported = self.devices.0.test()?;
        let second_supported = !self.config.contains(ConfigFlags::PORT_CLOCK_2) && self.devices.1.test()?;

        Ok((first_supported, second_supported))
    }

    /// Resets all devices and returns the count available
    fn reset_devices(&mut self) -> Result<u8, Ps2Error> {
        let mut available_count = 0;

        if self.devices.0.state == DeviceState::Available {
            self.devices.0.reset()?;
            available_count += 1;
        }

        if self.devices.1.state == DeviceState::Available {
            self.devices.1.reset()?;
            available_count += 1;
        }

        Ok(available_count)
    }
}

/// Represents a PS2 device
pub struct Device {
    state: DeviceState,
    port: DevicePort,
}

impl Device {
    const fn new(port: DevicePort) -> Self {
        Device {
            state: DeviceState::Unavailable,
            port,
        }
    }

    /// Tests this device to see if it is available
    pub fn test(&mut self) -> Result<bool, Ps2Error> {
        let cmd = if self.port == DevicePort::Mouse {
            ControllerReturnCommand::TestPort2
        } else {
            ControllerReturnCommand::TestPort1
        };
        let available = io::command_ret(cmd)? == 0x0;

        if available {
            self.state = DeviceState::Available;
        } else {
            self.state = DeviceState::Unavailable;
        }

        Ok(available)
    }

    /// Enables this device
    pub fn enable(&mut self) -> Result<(), Ps2Error> {
        let cmd = if self.port == DevicePort::Mouse {
            ControllerCommand::EnablePort2
        } else {
            ControllerCommand::EnablePort1
        };
        io::command(cmd)?;
        self.state = DeviceState::Enabled;

        Ok(())
    }

    /// Disables this device
    pub fn disable(&mut self) -> Result<(), Ps2Error> {
        let cmd = if self.port == DevicePort::Mouse {
            ControllerCommand::DisablePort2
        } else {
            ControllerCommand::DisablePort1
        };
        io::command(cmd)?;
        self.state = DeviceState::Available;

        Ok(())
    }

    /// Resets this device
    pub fn reset(&mut self) -> Result<(), Ps2Error> {
        self.command(DeviceCommand::Reset)?;

        Ok(())
    }

    /// Sends a command for this PS2 device and returns result
    pub fn command(&mut self, cmd: DeviceCommand) -> Result<u8, Ps2Error> {
        if self.state != DeviceState::Unavailable {
            // If second PS2 port, send context switch command
            if self.port == DevicePort::Mouse {
                io::command(ControllerCommand::WriteInputPort2)?;
            }
            for _ in 0..4 {
                io::write(&io::DATA_PORT, cmd as u8)?;
                match io::read(&io::DATA_PORT) {
                    Ok(RESEND) => continue,
                    result => return result,
                }
            }
            Err(Ps2Error::NoData)
        } else {
            Err(Ps2Error::DeviceUnavailable)
        }
    }

    /// Sends a command for this PS2 device with data and returns a result
    #[allow(dead_code)] // To be used by drivers interfacing with PS/2
    pub fn command_data(&mut self, cmd: DeviceCommand, data: u8) -> Result<u8, Ps2Error> {
        if self.state != DeviceState::Unavailable {
            self.command(cmd).and_then(|result| match result {
                ACK => {
                    io::write(&io::DATA_PORT, data as u8)?;
                    io::read(&io::DATA_PORT)
                }
                _ => Ok(result)
            })
        } else {
            Err(Ps2Error::DeviceUnavailable)
        }
    }
}

/// Represents the state of a device.
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum DeviceState {
    /// The device does not exist or hasn't been detected yet
    Unavailable,
    /// The device has been detected but is not enabled
    Available,
    /// The device has been enabled
    Enabled,
}

/// Represents the port of a device
#[derive(Copy, Clone, Eq, PartialEq)]
pub enum DevicePort {
    /// The device is in the keyboard port
    Keyboard(ControllerCommand::EnablePort1),
    /// The device is in the mouse port
    Mouse(ControllerCommand::EnablePort2),
}
