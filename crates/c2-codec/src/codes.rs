use c2_model::{Domain, FunctionalService};

use crate::CodecError;

pub fn domain_code(domain: &Domain) -> &'static str {
    match domain {
        Domain::Land => "LAND",
        Domain::Air => "AIR",
        Domain::Maritime => "MARITIME",
        Domain::Space => "SPACE",
        Domain::Cyber => "CYBER",
    }
}

pub fn domain_from(code: &str) -> Result<Domain, CodecError> {
    match code.trim() {
        "LAND" => Ok(Domain::Land),
        "AIR" => Ok(Domain::Air),
        "MARITIME" => Ok(Domain::Maritime),
        "SPACE" => Ok(Domain::Space),
        "CYBER" => Ok(Domain::Cyber),
        other => Err(CodecError::Decode(format!(
            "unknown operational domain {other}"
        ))),
    }
}

pub fn service_code(service: &FunctionalService) -> &'static str {
    match service {
        FunctionalService::CommandAndControl => "COMMAND_AND_CONTROL",
        FunctionalService::Intelligence => "INTELLIGENCE",
        FunctionalService::Logistics => "LOGISTICS",
        FunctionalService::Medical => "MEDICAL",
    }
}

pub fn service_from(code: &str) -> Result<FunctionalService, CodecError> {
    match code.trim() {
        "COMMAND_AND_CONTROL" => Ok(FunctionalService::CommandAndControl),
        "INTELLIGENCE" => Ok(FunctionalService::Intelligence),
        "LOGISTICS" => Ok(FunctionalService::Logistics),
        "MEDICAL" => Ok(FunctionalService::Medical),
        other => Err(CodecError::Decode(format!(
            "unknown functional service {other}"
        ))),
    }
}
