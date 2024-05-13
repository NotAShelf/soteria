use authority::{AuthorityProxy, Subject};
use dbus::AuthenticationAgent;
use gtk::glib::{self, clone, spawn_future_local, SignalHandlerId};
use std::cell::RefCell;
use std::collections::HashMap;
use tokio::sync::broadcast::channel;
use zbus::zvariant::Value;

use gtk::{gio::Cancellable, prelude::*, Builder};
use gtk::{
    Application, ApplicationWindow, Button, DropDown, Label, PasswordEntry, StringList,
    StringObject,
};
use gtk4 as gtk;
use zbus::conn;

use crate::events::AuthenticationEvent;

mod authority;
mod constants;
mod dbus;
mod events;
mod ui;

#[tokio::main]
async fn main() {
    let subscriber = tracing_subscriber::fmt()
        .with_target(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive(
                "zbus::object_server::ObjectServer::start_object_server[start_object_server]=debug"
                    .parse()
                    .unwrap(),
            ),
        )
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    match real_main().await {
        Ok(_) => (),
        Err(e) => {
            tracing::error!("A fatal error occurred when running the application: {}", e);
            std::process::exit(1);
        }
    }
}
async fn real_main() -> Result<(), Box<dyn std::error::Error>> {
    gtk::init()?;

    let application = Application::builder()
        .application_id("gay.vaskel.Soteria")
        .build();

    let builder = Builder::from_string(constants::UI_XML);

    let window: ApplicationWindow = ui::get_object(&builder, "window")?;
    let password_entry: PasswordEntry = ui::get_object(&builder, "password-entry")?;
    let cancel_button: Button = ui::get_object(&builder, "cancel-button")?;
    let confirm_button: Button = ui::get_object(&builder, "confirm-button")?;
    let info_label: Label = ui::get_object(&builder, "label-message")?;

    let w2 = window.clone();
    application.connect_activate(move |app| {
        app.add_window(&w2);
    });

    let (tx, mut rx) = channel::<AuthenticationEvent>(100);

    let locale = "en_US.UTF-8";
    let subject_kind = "unix-session".to_string();
    let subject_details = HashMap::from([(
        "session-id".to_string(),
        Value::new(std::env::var("XDG_SESSION_ID")?),
    )]);
    let subject = Subject::new(subject_kind, subject_details);

    application.register(Cancellable::NONE)?;
    application.activate();

    let agent = AuthenticationAgent::new(tx.clone());
    let connection = conn::Builder::system()?
        .serve_at(constants::SELF_OBJECT_PATH, agent)?
        .build()
        .await?;

    let proxy = AuthorityProxy::new(&connection).await?;
    proxy
        .register_authentication_agent(&subject, locale, constants::SELF_OBJECT_PATH)
        .await?;

    tracing::info!("Registered as authentication provider.");

    spawn_future_local(clone!(@weak window, @weak builder => async move {
        let mut current_cookie: Option<String> = None;
        let mut current_listeners: RefCell<Option<(SignalHandlerId, SignalHandlerId)>> = RefCell::new(None);

        loop {
            let dropdown: DropDown = builder.object("identity-dropdown").unwrap();
            let failed_alert = ui::build_fail_alert();

            let event = rx.recv().await.expect("Somehow the channel closed.");
            tracing::debug!("recieved event {:#?}", event);

            match event {
                AuthenticationEvent::Started{cookie, message, names} => {
                    if current_cookie.as_ref().is_some_and(|c| c != &cookie) {
                            tx.send(AuthenticationEvent::AlreadyRunning{cookie}).unwrap();
                            continue;
                    }

                    let store: StringList = builder.object("identity-dropdown-values").unwrap();
                    for name in names.iter() {
                        store.append(name.as_str());
                    }
                    info_label.set_label(&message);

                    let cancel_listener = cancel_button.connect_clicked(clone!(@weak window, @weak password_entry, @weak info_label, @strong cookie, @strong tx => move |_| {
                        tx.send(AuthenticationEvent::UserCanceled{cookie: cookie.clone()}).unwrap();
                        password_entry.set_text("");
                        info_label.set_text("");
                        window.set_visible(false);
                    }));

                    let confirm_listener = confirm_button.connect_clicked(clone!(@weak window, @weak password_entry, @weak info_label, @strong cookie, @strong tx => move |_| {
                        let pw = password_entry.text();
                        let user: StringObject = dropdown.selected_item().unwrap().dynamic_cast().unwrap();
                        tx.send(AuthenticationEvent::UserProvidedPassword { cookie: cookie.clone(), username: user.string().to_string(), password: pw.to_string()}).unwrap();
                        password_entry.set_text("");
                        info_label.set_text("");
                        window.set_visible(false);
                    }));

                    current_listeners = RefCell::new(Some((confirm_listener, cancel_listener)));
                    current_cookie = Some(cookie.clone());
                    tracing::debug!("Attempting to prompt user for authentication.");
                    window.present();
                }
                AuthenticationEvent::Canceled{cookie: c} => {
                    if current_cookie.as_ref().is_some_and(|cc| cc == &c) {
                        current_cookie = None;
                        if let Some((con, can)) = current_listeners.take() {
                            cancel_button.disconnect(can);
                            confirm_button.disconnect(con);
                        }
                        window.set_visible(false);
                    }


                },
                AuthenticationEvent::UserProvidedPassword{ cookie: c, username: _, password: _} => {
                    if current_cookie.as_ref().is_some_and(|cc| cc == &c) {
                        current_cookie = None;
                        if let Some((con, can)) = current_listeners.take() {
                            cancel_button.disconnect(can);
                            confirm_button.disconnect(con);
                        }
                        window.set_visible(false);
                    }

                }
                AuthenticationEvent::AuthorizationFailed{cookie: _} => {
                    failed_alert.show(Some(&window));
                }
                _ => (),
            }
        }
    }));

    application.run();

    Ok(())
}
