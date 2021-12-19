/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

use mozjs::jsapi::JSFunctionSpec;

use ion::error::IonError;
use ion::functions::function::IonFunction;
use ion::IonContext;
use ion::objects::object::{IonObject, JSPROP_CONSTANT};

use crate::event_loop::EVENT_LOOP;
use crate::event_loop::microtasks::Microtask;

#[js_fn]
fn queueMicrotask(cx: IonContext, callback: IonFunction) -> IonResult<()> {
	EVENT_LOOP.with(|event_loop| {
		if let Some(queue) = (*event_loop.borrow()).microtasks.clone() {
			queue.enqueue(cx, Microtask::User(callback));
			Ok(())
		} else {
			Err(IonError::Error(String::from("Microtask Queue has not been initialised.")))
		}
	})
}

const FUNCTION: JSFunctionSpec = function_spec!(queueMicrotask, 0);

pub unsafe fn define(cx: IonContext, mut global: IonObject) -> bool {
	global.define_as(cx, "queueMicrotask", IonFunction::from_spec(cx, &FUNCTION), JSPROP_CONSTANT as u32)
}
