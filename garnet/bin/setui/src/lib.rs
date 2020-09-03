// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This declaration is required to support the `select!`.
#![recursion_limit = "256"]

use {
    crate::accessibility::accessibility_controller::AccessibilityController,
    crate::account::account_controller::AccountController,
    crate::agent::authority_impl::AuthorityImpl,
    crate::agent::base::{Authority, BlueprintHandle as AgentBlueprintHandle, Lifespan},
    crate::audio::audio_controller::AudioController,
    crate::config::base::ControllerFlag,
    crate::device::device_controller::DeviceController,
    crate::display::display_controller::{DisplayController, ExternalBrightnessControl},
    crate::display::light_sensor_controller::LightSensorController,
    crate::do_not_disturb::do_not_disturb_controller::DoNotDisturbController,
    crate::handler::base::GenerateHandler,
    crate::handler::device_storage::DeviceStorageFactory,
    crate::handler::setting_handler::persist::Handler as DataHandler,
    crate::handler::setting_handler::Handler,
    crate::handler::setting_handler_factory_impl::SettingHandlerFactoryImpl,
    crate::handler::setting_proxy::SettingProxy,
    crate::input::input_controller::InputController,
    crate::inspect::inspect_broker::InspectBroker,
    crate::intl::intl_controller::IntlController,
    crate::light::light_controller::LightController,
    crate::night_mode::night_mode_controller::NightModeController,
    crate::power::power_controller::PowerController,
    crate::privacy::privacy_controller::PrivacyController,
    crate::service_context::GenerateService,
    crate::service_context::ServiceContext,
    crate::service_context::ServiceContextHandle,
    crate::setup::setup_controller::SetupController,
    crate::switchboard::accessibility_types::AccessibilityInfo,
    crate::switchboard::base::{
        AudioInfo, DisplayInfo, DoNotDisturbInfo, InputInfo, NightModeInfo, PrivacyInfo,
        SettingType, SetupInfo,
    },
    crate::switchboard::intl_types::IntlInfo,
    crate::switchboard::light_types::LightInfo,
    crate::switchboard::switchboard::SwitchboardBuilder,
    anyhow::{format_err, Error},
    fidl_fuchsia_settings::*,
    fuchsia_async as fasync,
    fuchsia_component::server::{NestedEnvironment, ServiceFs, ServiceFsDir, ServiceObj},
    fuchsia_inspect::component,
    fuchsia_syslog::fx_log_err,
    fuchsia_zircon::DurationNum,
    futures::lock::Mutex,
    futures::StreamExt,
    serde::Deserialize,
    std::collections::{HashMap, HashSet},
    std::sync::Arc,
};

mod accessibility;
mod account;
mod audio;
mod clock;
mod device;
mod display;
mod do_not_disturb;
mod fidl_clone;
mod fidl_processor;
mod input;
mod inspect;
mod internal;
mod intl;
mod light;
mod night_mode;
mod power;
mod privacy;
mod setup;

pub use display::LightSensorConfig;
pub use light::light_hardware_configuration::LightHardwareConfiguration;

pub mod agent;
pub mod config;
pub mod fidl_common;
pub mod handler;
pub mod message;
pub mod service_context;
pub mod switchboard;

const DEFAULT_SETTING_PROXY_MAX_ATTEMPTS: u64 = 3;
const DEFAULT_SETTING_PROXY_RESPONSE_TIMEOUT_MS: i64 = 10_000;

/// A common trigger for exiting.
pub type ExitSender = futures::channel::mpsc::UnboundedSender<()>;

/// Runtime defines where the environment will exist. Service is meant for
/// production environments and will hydrate components to be discoverable as
/// an environment service. Nested creates a service only usable in the scope
/// of a test.
#[derive(PartialEq)]
enum Runtime {
    Service,
    Nested(&'static str),
}

#[derive(PartialEq, Debug, Clone, Deserialize)]
pub struct EnabledServicesConfiguration {
    pub services: HashSet<SettingType>,
}

impl EnabledServicesConfiguration {
    pub fn with_services(services: HashSet<SettingType>) -> Self {
        Self { services }
    }
}

#[derive(Default, Debug, Clone, Deserialize)]
pub struct ServiceFlags {
    pub controller_flags: HashSet<ControllerFlag>,
}

#[derive(PartialEq, Debug, Clone)]
pub struct ServiceConfiguration {
    pub services: HashSet<SettingType>,
    pub controller_flags: HashSet<ControllerFlag>,
}

impl ServiceConfiguration {
    pub fn from(services: EnabledServicesConfiguration, flags: ServiceFlags) -> Self {
        Self { services: services.services, controller_flags: flags.controller_flags }
    }
}

/// Environment is handed back when an environment is spawned from the
/// EnvironmentBuilder. A nested environment (if available) is returned,
/// along with a receiver to be notified when initialization/setup is
/// complete.
pub struct Environment {
    pub nested_environment: Option<NestedEnvironment>,
}

impl Environment {
    pub fn new(nested_environment: Option<NestedEnvironment>) -> Environment {
        Environment { nested_environment }
    }
}

/// The EnvironmentBuilder aggregates the parameters surrounding an environment
/// and ultimately spawns an environment based on them.
pub struct EnvironmentBuilder<T: DeviceStorageFactory + Send + Sync + 'static> {
    configuration: Option<ServiceConfiguration>,
    agent_blueprints: Vec<AgentBlueprintHandle>,
    event_subscriber_blueprints: Vec<internal::event::subscriber::BlueprintHandle>,
    storage_factory: Arc<Mutex<T>>,
    generate_service: Option<GenerateService>,
    handlers: HashMap<SettingType, GenerateHandler<T>>,
}

macro_rules! register_handler {
    ($handler_factory:ident, $setting_type:expr, $spawn_method:expr) => {
        $handler_factory.register($setting_type, Box::new($spawn_method));
    };
}

/// This macro conditionally adds a FIDL service handler based on the presence
/// of `SettingType`s in the available components. The caller specifies the
/// mod containing a generated fidl_io mod to handle the incoming request
/// streams, the target FIDL interface, and a list of `SettingType`s whose
/// presence will cause this handler to be included.
macro_rules! register_fidl_handler {
    ($components:ident, $service_dir:ident, $messenger_factory:ident,
            $interface:ident, $handler_mod:ident$(, $target:ident)+) => {
        if false $(|| $components.contains(&SettingType::$target))+
        {
            let factory = $messenger_factory.clone();
            $service_dir.add_fidl_service(move |stream: paste::item!{[<$interface RequestStream>]}| {
                crate::$handler_mod::fidl_io::spawn(factory.clone(), stream);
            });
        }
    }
}

impl<T: DeviceStorageFactory + Send + Sync + 'static> EnvironmentBuilder<T> {
    pub fn new(storage_factory: Arc<Mutex<T>>) -> EnvironmentBuilder<T> {
        EnvironmentBuilder {
            configuration: None,
            agent_blueprints: vec![],
            event_subscriber_blueprints: vec![],
            storage_factory,
            generate_service: None,
            handlers: HashMap::new(),
        }
    }

    pub fn handler(
        mut self,
        setting_type: SettingType,
        generate_handler: GenerateHandler<T>,
    ) -> EnvironmentBuilder<T> {
        self.handlers.insert(setting_type, generate_handler);
        self
    }

    /// A service generator to be used as an overlay on the ServiceContext.
    pub fn service(mut self, generate_service: GenerateService) -> EnvironmentBuilder<T> {
        self.generate_service = Some(generate_service);
        self
    }

    /// A preset configuration to load preset parameters as a base.
    pub fn configuration(mut self, configuration: ServiceConfiguration) -> EnvironmentBuilder<T> {
        self.configuration = Some(configuration);
        self
    }

    /// Setting types to participate.
    pub fn settings(mut self, settings: &[SettingType]) -> EnvironmentBuilder<T> {
        let controller_flags =
            self.configuration.take().map(|c| c.controller_flags).unwrap_or_else(|| HashSet::new());
        self.configuration(ServiceConfiguration {
            services: settings.to_vec().into_iter().collect(),
            controller_flags,
        })
    }

    /// Setting types to participate with customized controllers.
    pub fn flags(mut self, controller_flags: &[ControllerFlag]) -> EnvironmentBuilder<T> {
        let services =
            self.configuration.take().map(|c| c.services).unwrap_or_else(|| HashSet::new());
        self.configuration(ServiceConfiguration {
            services,
            controller_flags: controller_flags.iter().map(|f| *f).collect(),
        })
    }

    pub fn agents(mut self, blueprints: &[AgentBlueprintHandle]) -> EnvironmentBuilder<T> {
        self.agent_blueprints.append(&mut blueprints.to_vec());
        self
    }

    /// Event subscribers to participate
    pub fn event_subscribers(
        mut self,
        subscribers: &[internal::event::subscriber::BlueprintHandle],
    ) -> EnvironmentBuilder<T> {
        self.event_subscriber_blueprints.append(&mut subscribers.to_vec());
        self
    }

    async fn prepare_env(
        self,
        runtime: Runtime,
    ) -> Result<ServiceFs<ServiceObj<'static, ()>>, Error> {
        let mut fs = ServiceFs::new();

        let service_dir;
        if runtime == Runtime::Service {
            // Initialize inspect.
            component::inspector().serve(&mut fs).ok();

            service_dir = fs.dir("svc");
        } else {
            service_dir = fs.root_dir();
        }

        let (settings, flags) = match self.configuration {
            Some(configuration) => (configuration.services, configuration.controller_flags),
            _ => (HashSet::new(), HashSet::new()),
        };

        let event_messenger_factory = internal::event::message::create_hub();
        let service_context =
            ServiceContext::create(self.generate_service, Some(event_messenger_factory.clone()));

        let mut handler_factory = SettingHandlerFactoryImpl::new(
            settings.clone(),
            service_context.clone(),
            self.storage_factory.clone(),
        );

        EnvironmentBuilder::get_configuration_handlers(&flags, &mut handler_factory);

        // Override the configuration handlers with any custom handlers specified
        // in the environment.
        for (setting_type, handler) in self.handlers {
            handler_factory.register(setting_type, handler);
        }

        if create_environment(
            service_dir,
            settings,
            self.agent_blueprints,
            self.event_subscriber_blueprints,
            service_context,
            event_messenger_factory,
            Arc::new(Mutex::new(handler_factory)),
        )
        .await
        .is_err()
        {
            return Err(format_err!("could not create environment"));
        }

        Ok(fs)
    }

    pub fn spawn(self, mut executor: fasync::Executor) -> Result<(), Error> {
        match executor.run_singlethreaded(self.prepare_env(Runtime::Service)) {
            Ok(mut fs) => {
                fs.take_and_serve_directory_handle().expect("could not service directory handle");
                let () = executor.run_singlethreaded(fs.collect());

                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    pub async fn spawn_nested(self, env_name: &'static str) -> Result<Environment, Error> {
        match self.prepare_env(Runtime::Nested(env_name)).await {
            Ok(mut fs) => {
                let nested_environment = Some(fs.create_salted_nested_environment(&env_name)?);
                fasync::Task::spawn(fs.collect()).detach();

                Ok(Environment::new(nested_environment))
            }
            Err(error) => Err(error),
        }
    }

    /// Spawns a nested environment and returns the associated
    /// NestedEnvironment. Note that this is a helper function that provides a
    /// shortcut for calling EnvironmentBuilder::name() and
    /// EnvironmentBuilder::spawn().
    pub async fn spawn_and_get_nested_environment(
        self,
        env_name: &'static str,
    ) -> Result<NestedEnvironment, Error> {
        let environment = self.spawn_nested(env_name).await?;

        if let Some(env) = environment.nested_environment {
            return Ok(env);
        }

        return Err(format_err!("nested environment not created"));
    }

    fn get_configuration_handlers(
        controller_flags: &HashSet<ControllerFlag>,
        factory_handle: &mut SettingHandlerFactoryImpl<T>,
    ) {
        // Power
        register_handler!(factory_handle, SettingType::Power, Handler::<PowerController>::spawn);
        // Accessibility
        register_handler!(
            factory_handle,
            SettingType::Accessibility,
            DataHandler::<AccessibilityInfo, AccessibilityController>::spawn
        );
        // Account
        register_handler!(
            factory_handle,
            SettingType::Account,
            Handler::<AccountController>::spawn
        );
        // Audio
        register_handler!(
            factory_handle,
            SettingType::Audio,
            DataHandler::<AudioInfo, AudioController>::spawn
        );
        // Device
        register_handler!(factory_handle, SettingType::Device, Handler::<DeviceController>::spawn);
        // Display
        register_handler!(
            factory_handle,
            SettingType::Display,
            if controller_flags.contains(&ControllerFlag::ExternalBrightnessControl) {
                DataHandler::<DisplayInfo, DisplayController<ExternalBrightnessControl>>::spawn
            } else {
                DataHandler::<DisplayInfo, DisplayController>::spawn
            }
        );
        // Light
        register_handler!(
            factory_handle,
            SettingType::Light,
            DataHandler::<LightInfo, LightController>::spawn
        );
        // Light sensor
        register_handler!(
            factory_handle,
            SettingType::LightSensor,
            Handler::<LightSensorController>::spawn
        );
        // Input
        register_handler!(
            factory_handle,
            SettingType::Input,
            DataHandler::<InputInfo, InputController>::spawn
        );
        // Intl
        register_handler!(
            factory_handle,
            SettingType::Intl,
            DataHandler::<IntlInfo, IntlController>::spawn
        );
        // Do not disturb
        register_handler!(
            factory_handle,
            SettingType::DoNotDisturb,
            DataHandler::<DoNotDisturbInfo, DoNotDisturbController>::spawn
        );
        // Night mode
        register_handler!(
            factory_handle,
            SettingType::NightMode,
            DataHandler::<NightModeInfo, NightModeController>::spawn
        );
        // Privacy
        register_handler!(
            factory_handle,
            SettingType::Privacy,
            DataHandler::<PrivacyInfo, PrivacyController>::spawn
        );
        // Setup
        register_handler!(
            factory_handle,
            SettingType::Setup,
            DataHandler::<SetupInfo, SetupController>::spawn
        );
    }
}

/// Brings up the settings service environment.
///
/// This method generates the necessary infrastructure to support the settings
/// service (switchboard, proxies, etc.) and brings up the components necessary
/// to support the components specified in the components HashSet.
async fn create_environment<'a, T: DeviceStorageFactory + Send + Sync + 'static>(
    mut service_dir: ServiceFsDir<'_, ServiceObj<'a, ()>>,
    components: HashSet<SettingType>,
    agent_blueprints: Vec<AgentBlueprintHandle>,
    event_subscriber_blueprints: Vec<internal::event::subscriber::BlueprintHandle>,
    service_context_handle: ServiceContextHandle,
    event_messenger_factory: internal::event::message::Factory,
    handler_factory: Arc<Mutex<SettingHandlerFactoryImpl<T>>>,
) -> Result<(), Error> {
    let core_messenger_factory = internal::core::message::create_hub();
    let switchboard_messenger_factory = internal::switchboard::message::create_hub();
    let setting_handler_messenger_factory = internal::handler::message::create_hub();

    for blueprint in event_subscriber_blueprints {
        blueprint.create(event_messenger_factory.clone()).await;
    }

    // Attach inspect broker, which watches messages between proxies and setting handlers to
    // record settings values to inspect.
    let inspect_broker_node = component::inspector().root().create_child("setting_values");
    InspectBroker::create(setting_handler_messenger_factory.clone(), inspect_broker_node)
        .await
        .expect("could not create inspect");

    let mut proxies = HashMap::new();

    // TODO(58893): make max attempts a configurable option.
    // TODO(59174): make setting proxy response timeout and retry configurable.
    for setting_type in &components {
        proxies.insert(
            *setting_type,
            SettingProxy::create(
                *setting_type,
                handler_factory.clone(),
                core_messenger_factory.clone(),
                setting_handler_messenger_factory.clone(),
                event_messenger_factory.clone(),
                DEFAULT_SETTING_PROXY_MAX_ATTEMPTS,
                Some(DEFAULT_SETTING_PROXY_RESPONSE_TIMEOUT_MS.millis()),
                true,
            )
            .await?
            .0,
        );
    }

    // Creates switchboard, handed to interface implementations to send messages
    // to handlers.
    SwitchboardBuilder::create()
        .core_messenger_factory(core_messenger_factory)
        .switchboard_messenger_factory(switchboard_messenger_factory.clone())
        .add_setting_proxies(proxies)
        .build()
        .await
        .expect("could not create switchboard");

    let mut agent_authority = AuthorityImpl::create(
        internal::agent::message::create_hub(),
        switchboard_messenger_factory.clone(),
        event_messenger_factory.clone(),
        components.clone(),
    )
    .await?;

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Light,
        light,
        Light
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Accessibility,
        accessibility,
        Accessibility
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Audio,
        audio,
        Audio
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Device,
        device,
        Device
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Display,
        display,
        Display,
        LightSensor
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        DoNotDisturb,
        do_not_disturb,
        DoNotDisturb
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Intl,
        intl,
        Intl
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        NightMode,
        night_mode,
        NightMode
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Privacy,
        privacy,
        Privacy
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Input,
        input,
        Input
    );

    register_fidl_handler!(
        components,
        service_dir,
        switchboard_messenger_factory,
        Setup,
        setup,
        Setup
    );

    for blueprint in agent_blueprints {
        if agent_authority.register(blueprint).await.is_err() {
            fx_log_err!("failed to register agent via blueprint");
        }
    }

    // Execute initialization agents sequentially
    if agent_authority
        .execute_lifespan(Lifespan::Initialization, service_context_handle.clone(), true)
        .await
        .is_err()
    {
        return Err(format_err!("Agent initialization failed"));
    }

    // Execute service agents concurrently
    agent_authority
        .execute_lifespan(Lifespan::Service, service_context_handle.clone(), false)
        .await
        .ok();

    return Ok(());
}

#[cfg(test)]
mod tests;
