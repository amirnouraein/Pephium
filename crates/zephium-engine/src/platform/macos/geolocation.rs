//! CoreLocation position source for WebKit geolocation.
//!
//! On macOS WebKit only ships a CoreLocation provider for iOS; an embedder
//! must hand positions to each process pool through the C geolocation SPI.
//! WebKit calls `start_updating` only after the page-permission broker has
//! granted the origin, so the system location prompt never precedes the
//! browser's own per-site prompt.

use std::cell::RefCell;
use std::ffi::c_void;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
use objc2_core_location::{
    kCLErrorDomain, kCLLocationAccuracyBest, kCLLocationAccuracyHundredMeters,
    CLAuthorizationStatus, CLError, CLLocation, CLLocationManager, CLLocationManagerDelegate,
};
use objc2_foundation::{NSArray, NSError, NSObjectProtocol};
use objc2_web_kit::WKWebView;

type ManagerRef = *const c_void;

#[repr(C)]
struct ProviderV1 {
    version: i32,
    client_info: *const c_void,
    start_updating: extern "C" fn(ManagerRef, *const c_void),
    stop_updating: extern "C" fn(ManagerRef, *const c_void),
    set_enable_high_accuracy: extern "C" fn(ManagerRef, bool, *const c_void),
}

// SAFETY: immutable function pointers only; WebKit copies the struct.
unsafe impl Sync for ProviderV1 {}

static PROVIDER: ProviderV1 = ProviderV1 {
    version: 1,
    client_info: std::ptr::null(),
    start_updating,
    stop_updating,
    set_enable_high_accuracy,
};

#[link(name = "WebKit", kind = "framework")]
unsafe extern "C" {
    fn WKContextGetGeolocationManager(context: *const c_void) -> ManagerRef;
    fn WKGeolocationManagerSetProvider(manager: ManagerRef, provider: *const ProviderV1);
    fn WKGeolocationManagerProviderDidChangePosition(manager: ManagerRef, position: *const c_void);
    fn WKGeolocationManagerProviderDidFailToDeterminePosition(manager: ManagerRef);
    #[allow(clippy::too_many_arguments)]
    fn WKGeolocationPositionCreate_c(
        timestamp: f64,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
        provides_altitude: bool,
        altitude: f64,
        provides_altitude_accuracy: bool,
        altitude_accuracy: f64,
        provides_heading: bool,
        heading: f64,
        provides_speed: bool,
        speed: f64,
        provides_floor_level: bool,
        floor_level: f64,
    ) -> *const c_void;
    fn WKRetain(object: *const c_void) -> *const c_void;
    fn WKRelease(object: *const c_void);
}

/// Points the view's process pool at the shared CoreLocation source.
/// Repeating it for every view of a pool only rewrites the same client.
pub(crate) fn install(webview: &WKWebView) {
    // SAFETY: main-thread WebKit access. A Cocoa `WKProcessPool` is the
    // bridged wrapper of its `WKContextRef`, which the C SPI accepts as is.
    unsafe {
        let configuration = webview.configuration();
        let pool: Retained<NSObject> = msg_send![&*configuration, processPool];
        let manager = WKContextGetGeolocationManager(Retained::as_ptr(&pool).cast());
        if !manager.is_null() {
            WKGeolocationManagerSetProvider(manager, &PROVIDER);
        }
    }
}

/// One CoreLocation fix in the shape WebKit's position constructor takes.
/// CoreLocation marks unknown fields with negative values.
#[derive(Debug, PartialEq)]
struct Fix {
    timestamp: f64,
    latitude: f64,
    longitude: f64,
    accuracy: f64,
    altitude: Option<(f64, f64)>,
    heading: Option<f64>,
    speed: Option<f64>,
}

impl Fix {
    #[allow(clippy::too_many_arguments)]
    fn new(
        timestamp: f64,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
        altitude: f64,
        altitude_accuracy: f64,
        heading: f64,
        speed: f64,
    ) -> Option<Self> {
        (accuracy >= 0.0).then_some(Self {
            timestamp,
            latitude,
            longitude,
            accuracy,
            altitude: (altitude_accuracy >= 0.0).then_some((altitude, altitude_accuracy)),
            heading: (heading >= 0.0).then_some(heading),
            speed: (speed >= 0.0).then_some(speed),
        })
    }

    fn from_location(location: &CLLocation) -> Option<Self> {
        // SAFETY: plain property reads on a CoreLocation value object.
        unsafe {
            let coordinate = location.coordinate();
            Self::new(
                location.timestamp().timeIntervalSince1970(),
                coordinate.latitude,
                coordinate.longitude,
                location.horizontalAccuracy(),
                location.altitude(),
                location.verticalAccuracy(),
                location.course(),
                location.speed(),
            )
        }
    }

    fn send(&self, manager: ManagerRef) {
        let (altitude, altitude_accuracy) = self.altitude.unwrap_or_default();
        // SAFETY: `manager` is retained in `State::managers`; the position
        // is a +1 WebKit object released after WebKit copies it.
        unsafe {
            let position = WKGeolocationPositionCreate_c(
                self.timestamp,
                self.latitude,
                self.longitude,
                self.accuracy,
                self.altitude.is_some(),
                altitude,
                self.altitude.is_some(),
                altitude_accuracy,
                self.heading.is_some(),
                self.heading.unwrap_or_default(),
                self.speed.is_some(),
                self.speed.unwrap_or_default(),
                false,
                0.0,
            );
            WKGeolocationManagerProviderDidChangePosition(manager, position);
            WKRelease(position);
        }
    }
}

#[derive(Default)]
struct State {
    /// Process pools that currently want positions, each retained once.
    managers: Vec<ManagerRef>,
    location: Option<(Retained<CLLocationManager>, Retained<LocationDelegate>)>,
    last: Option<Fix>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn fail_all() {
    STATE.with_borrow(|state| {
        for manager in &state.managers {
            // SAFETY: retained manager, main thread.
            unsafe { WKGeolocationManagerProviderDidFailToDeterminePosition(*manager) };
        }
    });
}

extern "C" fn start_updating(manager: ManagerRef, _: *const c_void) {
    let Some(mtm) = MainThreadMarker::new() else {
        // SAFETY: WebKit passed a live manager.
        unsafe { WKGeolocationManagerProviderDidFailToDeterminePosition(manager) };
        return;
    };
    STATE.with_borrow_mut(|state| {
        if !state.managers.contains(&manager) {
            // SAFETY: balanced by `WKRelease` in `stop_updating`.
            state.managers.push(unsafe { WKRetain(manager) });
        }
        if let Some(fix) = &state.last {
            fix.send(manager);
        }
        let (location, _) = state.location.get_or_insert_with(|| {
            let delegate = LocationDelegate::new(mtm);
            // SAFETY: main-thread CoreLocation setup; the delegate is kept
            // alive beside the manager, which holds it weakly.
            let location = unsafe {
                let location = CLLocationManager::new();
                location.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                location.setDesiredAccuracy(kCLLocationAccuracyHundredMeters);
                location
            };
            (location, delegate)
        });
        // SAFETY: main-thread CoreLocation calls. The first request shows
        // the system prompt that `NSLocationUsageDescription` labels.
        unsafe {
            if location.authorizationStatus() == CLAuthorizationStatus::NotDetermined {
                location.requestWhenInUseAuthorization();
            }
            location.startUpdatingLocation();
        }
    });
}

extern "C" fn stop_updating(manager: ManagerRef, _: *const c_void) {
    STATE.with_borrow_mut(|state| {
        if let Some(index) = state.managers.iter().position(|m| *m == manager) {
            // SAFETY: releases the retain taken in `start_updating`.
            unsafe { WKRelease(state.managers.swap_remove(index)) };
        }
        if state.managers.is_empty() {
            if let Some((location, _)) = &state.location {
                // SAFETY: main-thread CoreLocation call.
                unsafe { location.stopUpdatingLocation() };
            }
        }
    });
}

extern "C" fn set_enable_high_accuracy(_: ManagerRef, enabled: bool, _: *const c_void) {
    STATE.with_borrow(|state| {
        if let Some((location, _)) = &state.location {
            // SAFETY: main-thread CoreLocation calls.
            unsafe {
                location.setDesiredAccuracy(if enabled {
                    kCLLocationAccuracyBest
                } else {
                    kCLLocationAccuracyHundredMeters
                });
            }
        }
    });
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct LocationDelegate;

    unsafe impl NSObjectProtocol for LocationDelegate {}

    unsafe impl CLLocationManagerDelegate for LocationDelegate {
        #[unsafe(method(locationManager:didUpdateLocations:))]
        fn did_update_locations(&self, _: &CLLocationManager, locations: &NSArray<CLLocation>) {
            let Some(fix) = locations.lastObject().and_then(|l| Fix::from_location(&l)) else {
                return;
            };
            STATE.with_borrow_mut(|state| {
                for manager in &state.managers {
                    fix.send(*manager);
                }
                state.last = Some(fix);
            });
        }

        #[unsafe(method(locationManager:didFailWithError:))]
        fn did_fail(&self, _: &CLLocationManager, error: &NSError) {
            // `LocationUnknown` is transient: CoreLocation keeps trying.
            // SAFETY: reading an immutable framework constant.
            let unknown = &*error.domain() == unsafe { kCLErrorDomain }
                && error.code() == CLError::LocationUnknown.0;
            if !unknown {
                fail_all();
            }
        }

        #[unsafe(method(locationManagerDidChangeAuthorization:))]
        fn did_change_authorization(&self, manager: &CLLocationManager) {
            // SAFETY: main-thread CoreLocation call.
            let status = unsafe { manager.authorizationStatus() };
            if matches!(
                status,
                CLAuthorizationStatus::Denied | CLAuthorizationStatus::Restricted
            ) {
                fail_all();
            }
        }
    }
);

impl LocationDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(());
        // SAFETY: NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_core_location_fields_are_reported_as_absent() {
        assert_eq!(Fix::new(1.0, 2.0, 3.0, -1.0, 0.0, 0.0, 0.0, 0.0), None);
        assert_eq!(
            Fix::new(1.0, 48.85, 2.35, 25.0, 35.0, -1.0, -1.0, -1.0),
            Some(Fix {
                timestamp: 1.0,
                latitude: 48.85,
                longitude: 2.35,
                accuracy: 25.0,
                altitude: None,
                heading: None,
                speed: None,
            })
        );
        let moving = Fix::new(1.0, 0.0, 0.0, 5.0, 35.0, 3.0, 90.0, 1.5).unwrap();
        assert_eq!(moving.altitude, Some((35.0, 3.0)));
        assert_eq!(moving.heading, Some(90.0));
        assert_eq!(moving.speed, Some(1.5));
    }

    #[test]
    fn delegate_answers_core_location_callbacks() {
        use objc2::{sel, ClassType};
        let class = LocationDelegate::class();
        for selector in [
            sel!(locationManager:didUpdateLocations:),
            sel!(locationManager:didFailWithError:),
            sel!(locationManagerDidChangeAuthorization:),
        ] {
            assert!(class.responds_to(selector));
        }
    }
}
