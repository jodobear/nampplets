//! Host-owned surface descriptors, bindings, revisions, and typed actions.

mod actions;
mod binding;
mod descriptor;
mod revision;

pub use actions::{
    ActionError, ActionHandler, ActionOutcome, ActionRequest, ActionRouter, ActionRouterLimits,
    RoutedActionFact,
};
pub use binding::{
    Binding, BindingConsumer, BindingError, BindingLimits, BindingSnapshot, RendererId,
    RendererSlot, RendererSwap,
};
pub use descriptor::{
    ActionDescriptor, DescriptorError, DescriptorLimits, Fallback, InputDescriptor, ParsedSurface,
    Presentation, SurfaceDescriptor, SurfaceProfile, parse_descriptor,
};
pub use revision::{ApplyOutcome, SurfaceClientProjection, SurfaceFrame, SurfaceProjectionError};
