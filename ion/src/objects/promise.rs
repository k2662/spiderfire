/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/.
 */

use std::future::Future;
use std::mem::transmute;
use std::ops::Deref;

use futures::executor::block_on;
use libffi::high::ClosureOnce3;
use mozjs::jsapi::{
	AddPromiseReactions, GetPromiseID, GetPromiseResult, GetPromiseState, IsPromiseObject, JSContext, JSObject, NewPromiseObject, PromiseState,
	RejectPromise, ResolvePromise,
};
use mozjs::jsval::JSVal;
use mozjs::rust::{Handle, HandleObject, MutableHandle};

use crate::{Arguments, Context, Exception, Function, Local, Object, Value};
use crate::conversions::ToValue;
use crate::exception::ThrowException;
use crate::flags::PropertyFlags;

/// Represents a [Promise] in the JS Runtime.
///
/// Refer to [MDN](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Promise) for more details.
#[derive(Debug)]
pub struct Promise<'p> {
	promise: Local<'p, *mut JSObject>,
}

impl<'p> Promise<'p> {
	/// Creates a new [Promise] which resolves immediately and returns `void`.
	pub fn new<'cx>(cx: &'cx Context) -> Promise<'cx> {
		Promise {
			promise: cx.root_object(unsafe { NewPromiseObject(**cx, HandleObject::null().into()) }),
		}
	}

	/// Creates a new [Promise] with an executor.
	///
	/// The executor is a function that takes in two functions, `resolve` and `reject`.
	/// `resolve` and `reject` can be called with a [Value] to resolve or reject the promise with the given value.
	pub fn new_with_executor<'cx, F>(cx: &'cx Context, executor: F) -> Option<Promise<'cx>>
	where
		F: for<'cx2> FnOnce(&'cx2 Context, Function<'cx2>, Function<'cx2>) -> crate::Result<()> + 'static,
	{
		unsafe {
			let native = move |mut cx: *mut JSContext, argc: u32, vp: *mut JSVal| {
				let cx = Context::new(&mut cx);
				let args = Arguments::new(&cx, argc, vp);

				let resolve_obj = args.value(0).unwrap().to_object(&cx).into_local();
				let reject_obj = args.value(1).unwrap().to_object(&cx).into_local();
				let resolve = Function::from_object(&cx, &resolve_obj).unwrap();
				let reject = Function::from_object(&cx, &reject_obj).unwrap();

				match executor(&cx, resolve, reject) {
					Ok(()) => true as u8,
					Err(error) => {
						error.throw(&cx);
						false as u8
					}
				}
			};
			let closure = ClosureOnce3::new(native);
			let fn_ptr = transmute::<_, &unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>(closure.code_ptr());

			let function = Function::new(cx, "executor", Some(*fn_ptr), 2, PropertyFlags::empty());
			let executor = function.to_object(cx);
			let promise = NewPromiseObject(**cx, executor.handle().into());

			if !promise.is_null() {
				Some(Promise { promise: cx.root_object(promise) })
			} else {
				None
			}
		}
	}

	/// Creates a new [Promise] with a [Future].
	///
	/// The future is run to completion on the current thread and cannot interact with an asynchronous runtime.
	///
	///
	/// If the future returns an [Ok], the promise is resolved with the [JSVal] contained within.
	///
	/// If the future returns an [Err], the promise is rejected with the [JSVal] contained within.
	pub fn block_on_future<'cx, F, Output, Error>(cx: &'cx Context, future: F) -> Option<Promise<'cx>>
	where
		F: Future<Output = Result<Output, Error>> + 'static,
		Output: for<'cx2> ToValue<'cx2> + 'static,
		Error: for<'cx2> ToValue<'cx2> + 'static,
	{
		let mut future = Some(future);
		Promise::new_with_executor(cx, move |cx, resolve, reject| {
			let null = Object::null(cx);
			block_on(async {
				unsafe {
					let future = future.take().unwrap();
					match future.await {
						Ok(output) => {
							let value = output.as_value(cx);
							if let Err(Some(error)) = resolve.call(cx, &null, &[value]) {
								println!("{}", error.format(cx));
							}
						}
						Err(error) => {
							let value = error.as_value(cx);
							if let Err(Some(error)) = reject.call(cx, &null, &[value]) {
								println!("{}", error.format(cx));
							}
						}
					}
				}
			});
			Ok(())
		})
	}

	/// Creates a [Promise] from an object.
	pub fn from(object: Local<'p, *mut JSObject>) -> Option<Promise<'p>> {
		if Promise::is_promise(&object) {
			Some(Promise { promise: object })
		} else {
			None
		}
	}

	/// Creates a [Promise] from aj object
	///
	/// ### Safety
	/// Object must be a Promise.
	pub unsafe fn from_unchecked(object: Local<'p, *mut JSObject>) -> Promise<'p> {
		Promise { promise: object }
	}

	/// Returns the ID of the [Promise].
	pub fn id(&self) -> u64 {
		unsafe { GetPromiseID(self.handle().into()) }
	}

	/// Returns the state of the [Promise].
	///
	/// The state can be `Pending`, `Fulfilled` and `Rejected`.
	pub fn state(&self) -> PromiseState {
		unsafe { GetPromiseState(self.handle().into()) }
	}

	/// Returns the result of the [Promise].
	///
	/// ### Note
	/// Currently leads to a segmentation fault.
	pub fn result<'cx>(&self, cx: &'cx Context) -> Value<'cx> {
		Value::from(cx.root_value(unsafe { GetPromiseResult(self.handle().into()) }))
	}

	/// Adds Reactions to the [Promise]
	///
	/// `on_resolved` is similar to calling `.then()` on a promise.
	///
	/// `on_rejected` is similar to calling `.catch()` on a promise.
	pub fn add_reactions<'cx, T, C>(&mut self, cx: &'cx Context, on_resolved: T, on_rejected: C) -> bool
	where
		T: for<'cx2> FnOnce(&'cx2 Context, &Value<'cx2>) -> Result<Value<'cx2>, Exception> + 'static,
		C: for<'cx2> FnOnce(&'cx2 Context, &Value<'cx2>) -> Result<Value<'cx2>, Exception> + 'static,
	{
		let mut resolved = Object::null(cx);
		let mut rejected = Object::null(cx);

		let resolve = move |mut cx: *mut JSContext, argc: u32, vp: *mut JSVal| {
			let cx = Context::new(&mut cx);
			let mut args = Arguments::new(&cx, argc, vp);
			let value = args.value(0).unwrap();
			match on_resolved(&cx, value) {
				Ok(value) => {
					args.rval().handle_mut().set(**value);
					true as u8
				}
				Err(exception) => {
					exception.throw(&cx);
					false as u8
				}
			}
		};
		let closure = Box::new(ClosureOnce3::new(resolve));
		let fn_ptr = unsafe { transmute::<_, &unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>(closure.code_ptr()) };
		Box::into_raw(closure);
		let resolve = Function::new(cx, "resolve", Some(*fn_ptr), 1, PropertyFlags::CONSTANT_ENUMERATED);
		resolved.handle_mut().set(**resolve.to_object(cx));

		let reject = move |mut cx: *mut JSContext, argc: u32, vp: *mut JSVal| {
			let cx = Context::new(&mut cx);
			let mut args = Arguments::new(&cx, argc, vp);
			let value = args.value(0).unwrap();
			match on_rejected(&cx, value) {
				Ok(value) => {
					args.rval().handle_mut().set(**value);
					true as u8
				}
				Err(exception) => {
					exception.throw(&cx);
					false as u8
				}
			}
		};
		let closure = Box::new(ClosureOnce3::new(reject));
		let fn_ptr = unsafe { transmute::<_, &unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>(closure.code_ptr()) };
		Box::into_raw(closure);
		let reject = Function::new(cx, "reject", Some(*fn_ptr), 1, PropertyFlags::CONSTANT_ENUMERATED);
		rejected.handle_mut().set(**reject.to_object(cx));

		unsafe { AddPromiseReactions(**cx, self.handle().into(), resolved.handle().into(), rejected.handle().into()) }
	}

	pub fn then<'cx, F>(&mut self, cx: &'cx Context, on_resolved: F) -> bool
	where
		F: for<'cx2> FnOnce(&'cx2 Context, &Value<'cx2>) -> Result<Value<'cx2>, Exception> + 'static,
	{
		let mut resolved = Object::null(cx);
		let rejected = Object::null(cx);

		let resolve = move |mut cx: *mut JSContext, argc: u32, vp: *mut JSVal| {
			let cx = Context::new(&mut cx);
			let mut args = Arguments::new(&cx, argc, vp);
			let value = args.value(0).unwrap();
			match on_resolved(&cx, value) {
				Ok(value) => {
					args.rval().handle_mut().set(**value);
					true as u8
				}
				Err(exception) => {
					exception.throw(&cx);
					false as u8
				}
			}
		};
		let closure = Box::new(ClosureOnce3::new(resolve));
		let fn_ptr = unsafe { transmute::<_, &unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>(closure.code_ptr()) };
		Box::into_raw(closure);
		let resolve = Function::new(cx, "resolve", Some(*fn_ptr), 1, PropertyFlags::CONSTANT_ENUMERATED);
		resolved.handle_mut().set(**resolve.to_object(cx));

		unsafe { AddPromiseReactions(**cx, self.handle().into(), resolved.handle().into(), rejected.handle().into()) }
	}

	pub fn catch<'cx, F>(&mut self, cx: &'cx Context, on_rejected: F) -> bool
	where
		F: for<'cx2> FnOnce(&'cx2 Context, &Value<'cx2>) -> Result<Value<'cx2>, Exception> + 'static,
	{
		let resolved = Object::null(cx);
		let mut rejected = Object::null(cx);

		let reject = move |mut cx: *mut JSContext, argc: u32, vp: *mut JSVal| {
			let cx = Context::new(&mut cx);
			let mut args = Arguments::new(&cx, argc, vp);
			let value = args.value(0).unwrap();
			match on_rejected(&cx, value) {
				Ok(value) => {
					args.rval().handle_mut().set(**value);
					true as u8
				}
				Err(exception) => {
					exception.throw(&cx);
					false as u8
				}
			}
		};
		let closure = Box::new(ClosureOnce3::new(reject));
		let fn_ptr = unsafe { transmute::<_, &unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool>(closure.code_ptr()) };
		Box::into_raw(closure);
		let reject = Function::new(cx, "reject", Some(*fn_ptr), 1, PropertyFlags::CONSTANT_ENUMERATED);
		rejected.handle_mut().set(**reject.to_object(cx));

		unsafe { AddPromiseReactions(**cx, self.handle().into(), resolved.handle().into(), rejected.handle().into()) }
	}

	/// Adds Reactions to the [Promise]
	///
	/// `on_resolved` is similar to calling `.then()` on a promise.
	///
	/// `on_rejected` is similar to calling `.catch()` on a promise.
	pub fn add_reactions_native(&mut self, cx: &'_ Context, on_resolved: Option<Function<'_>>, on_rejected: Option<Function<'_>>) -> bool {
		let mut resolved = Object::null(cx);
		let mut rejected = Object::null(cx);
		if let Some(on_resolved) = on_resolved {
			resolved.handle_mut().set(**on_resolved.to_object(cx));
		}
		if let Some(on_rejected) = on_rejected {
			rejected.handle_mut().set(**on_rejected.to_object(cx));
		}
		unsafe { AddPromiseReactions(**cx, self.handle().into(), resolved.handle().into(), rejected.handle().into()) }
	}

	/// Resolves the [Promise] with the given [Value].
	pub fn resolve(&self, cx: &Context, value: &Value) -> bool {
		unsafe { ResolvePromise(**cx, self.handle().into(), value.handle().into()) }
	}

	/// Rejects the [Promise] with the given [Value].
	pub fn reject(&self, cx: &Context, value: &Value) -> bool {
		unsafe { RejectPromise(**cx, self.handle().into(), value.handle().into()) }
	}

	pub fn handle<'s>(&'s self) -> Handle<'s, *mut JSObject>
	where
		'p: 's,
	{
		self.promise.handle()
	}

	pub fn handle_mut<'s>(&'s mut self) -> MutableHandle<'s, *mut JSObject>
	where
		'p: 's,
	{
		self.promise.handle_mut()
	}

	/// Checks if a [*mut] [JSObject] is a promise.
	pub fn is_promise_raw(cx: &Context, object: *mut JSObject) -> bool {
		rooted!(in(**cx) let object = object);
		unsafe { IsPromiseObject(object.handle().into()) }
	}

	/// Checks if an object is a promise.
	pub fn is_promise(object: &Local<*mut JSObject>) -> bool {
		unsafe { IsPromiseObject(object.handle().into()) }
	}
}

impl<'p> Deref for Promise<'p> {
	type Target = Local<'p, *mut JSObject>;

	fn deref(&self) -> &Self::Target {
		&self.promise
	}
}
