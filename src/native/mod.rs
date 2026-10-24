//! Native replacements for the old Python and Node harnesses: process inspection and hygiene,
//! resource sampling, and (Windows) desktop UI automation. No external runtime needed.

pub mod hygiene;
mod ownership;
pub mod procs;
pub mod resource;
pub mod web;
