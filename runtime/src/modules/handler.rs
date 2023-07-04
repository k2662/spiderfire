/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

use mozjs::jsapi::JSFunctionSpec;

use ion::{Context, ErrorReport, Exception, Function, Promise, Value};

use crate::cache::map::transform_error_report_with_sourcemaps;

#[js_fn]
unsafe fn on_rejected<'cx>(cx: &'cx Context, value: Value<'cx>) {
	let exception = Exception::from_value(cx, &value);
	let mut report = ErrorReport::from_exception_with_error_stack(cx, exception);
	transform_error_report_with_sourcemaps(&mut report);

	Exception::clear(cx);
	println!("{}", report.format(cx));
}

static ON_REJECTED: JSFunctionSpec = function_spec!(on_rejected, "onRejected", 0);

pub fn add_handler_reactions<'cx>(cx: &'cx Context, promise: &mut Promise<'cx>) -> bool {
	let on_rejected = Function::from_spec(cx, &ON_REJECTED);
	promise.add_reactions_native(cx, None, Some(on_rejected))
}
