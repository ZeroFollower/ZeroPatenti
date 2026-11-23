use std::{cell::RefCell, path::PathBuf};

use jni::{
    JNIEnv, JavaVM,
    objects::{JObject, JString},
};
use slint::android::{
    AndroidApp,
    android_activity::{MainEvent, PollEvent},
};

thread_local! {
    static PAUSE: RefCell<Option<Box<dyn Fn()>>> = RefCell::default();
    static IMPORT_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static ACTIVITY: RefCell<Option<AndroidApp>> = const { RefCell::new(None) };
    static PICKED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn choose_bank() -> anyhow::Result<()> {
    ACTIVITY.with_borrow(|slot| {
        let app = slot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Activity non disponibile"))?;
        // android-activity owns both handles until android_main returns.
        let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast())? };
        let obj = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        vm.attach_current_thread()?
            .call_method(&obj, "chooseBank", "()V", &[])?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_it_zeropatenti_app_BankActivity_bankReady(
    mut env: JNIEnv,
    _: JObject,
    file: JString,
) -> jni::sys::jboolean {
    let Ok(path) = env.get_string(&file) else {
        return 0;
    };
    let path = PathBuf::from(String::from(path));
    slint::invoke_from_event_loop(move || {
        PICKED.with_borrow_mut(|slot| slot.push(path));
    })
    .is_ok()
    .into()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_it_zeropatenti_app_BankActivity_bankFigures(
    mut env: JNIEnv,
    _: JObject,
    file: JString,
) -> jni::sys::jstring {
    let result = (|| -> anyhow::Result<_> {
        let path = PathBuf::from(String::from(env.get_string(&file)?));
        let figures: std::collections::HashSet<_> =
            crate::storage::load_normalized_questions(&path)?
                .into_iter()
                .filter_map(|q| q.figura)
                .collect();
        Ok(env.new_string(serde_json::to_string(&figures)?)?.into_raw())
    })();
    result.unwrap_or(std::ptr::null_mut())
}

pub(crate) fn take_banks() -> Vec<PathBuf> {
    PICKED.take()
}

pub(crate) fn import_dir() -> Option<PathBuf> {
    IMPORT_DIR.with_borrow(Clone::clone)
}

pub(crate) fn on_background(callback: impl Fn() + 'static) {
    PAUSE.with_borrow_mut(|slot| *slot = Some(Box::new(callback)));
}

#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    let Some(directory) = app.internal_data_path() else {
        return;
    };
    if let Some(inbox) = app.external_data_path().map(|dir| dir.join("import")) {
        if std::fs::create_dir_all(&inbox).is_ok() {
            IMPORT_DIR.with_borrow_mut(|slot| *slot = Some(inbox));
        }
    }
    ACTIVITY.with_borrow_mut(|slot| *slot = Some(app.clone()));
    let init = slint::android::init_with_event_listener(app, |event| {
        if matches!(event, PollEvent::Main(MainEvent::Pause | MainEvent::Stop)) {
            PAUSE.with_borrow(|slot| {
                if let Some(callback) = slot {
                    callback();
                }
            });
        }
    });
    if init.is_ok() {
        if let Err(error) = crate::gui::run(&if directory.join("patenti.sqlite3").is_file() {
            directory.join("patenti.sqlite3")
        } else {
            directory.join("progressi.sqlite3")
        }) {
            eprintln!("ZeroPatenti: {error:#}");
        }
    }
    PAUSE.with_borrow_mut(|slot| *slot = None);
    ACTIVITY.with_borrow_mut(|slot| *slot = None);
}
