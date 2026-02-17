pub mod api;
pub mod local;
pub mod provider;

pub use provider::EmbeddingProvider;

#[cfg(test)]
mod tests;
