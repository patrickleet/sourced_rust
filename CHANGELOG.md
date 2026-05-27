### What's changed in v0.5.0

* feat: add microsvc handler message specs (by @patrickleet)

  Implements the first slice of [[specs/async-microsvc-transports]].

  Adds handler metadata, message envelopes, subscription planning, and projection handler envelope dispatch.

* refactor: derive handler specs during registration (by @patrickleet)

  Removes per-handler SPEC constants and has register_handlers! construct HandlerSpec values from COMMAND, EVENT, and EVENTS constants.

* refactor: remove microsvc command registration aliases (by @patrickleet)

  Drops the compatibility Service::command, Service::command_guarded, and Service::commands APIs and updates tests/docs to use HandlerSpec registration.

* feat: add microsvc fluent handler registration (by @patrickleet)

  Adds HandlerBuilder so command/event registration can use .handle(...) or .guarded(...), with envelope selection before registration.

* refactor: expose messages on microsvc context (by @patrickleet)

  Removes envelope input modes so handlers always get ctx.message() plus ctx.input::<T>() for JSON payload decoding.

* fix: key microsvc handlers by message kind (by @patrickleet)

* fix: normalize microsvc message metadata (by @patrickleet)

* fix: register inventory reserved saga handler as event (by @patrickleet)

* fix: register order completed saga handler as event (by @patrickleet)

* fix: register order created saga handler as event (by @patrickleet)

* fix: register payment succeeded saga handler as event (by @patrickleet)

* refactor: require explicit handler registration kind (by @patrickleet)


See full diff: [v0.4.0...v0.5.0](https://github.com/patrickleet/sourced_rust/compare/v0.4.0...v0.5.0)
