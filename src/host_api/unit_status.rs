use super::{
    HostApi, HostApiError, HostApiErrorDetail, HostApiOperation, HostApiResponse, UnitStatusEntry,
    UnitStatusRequest, UnitStatusValue, validate_event, validate_non_empty,
};
use crate::event::EventContext;

impl HostApi {
    pub fn unit_status(
        &self,
        event: &EventContext,
        request: UnitStatusRequest,
    ) -> Result<HostApiResponse<UnitStatusValue>, HostApiError> {
        validate_event(event, HostApiOperation::UnitStatus)?;
        if let Some(unit_id) = request.unit_id.as_deref() {
            validate_non_empty(unit_id, "unit_id", HostApiOperation::UnitStatus)?;
        }

        let registry = self.unit_registry(HostApiOperation::UnitStatus)?;
        let summary = registry.status_summary();
        let unit = if let Some(unit_id) = request.unit_id.clone() {
            let descriptor = registry.get(&unit_id).ok_or_else(|| {
                HostApiError::validation(
                    HostApiOperation::UnitStatus,
                    HostApiErrorDetail::UnknownUnit {
                        unit_id: unit_id.clone(),
                    },
                )
            })?;
            Some(UnitStatusEntry::from_descriptor(descriptor))
        } else {
            None
        };

        Ok(self.response(
            HostApiOperation::UnitStatus,
            UnitStatusValue {
                requested_unit_id: request.unit_id,
                summary,
                unit,
            },
        ))
    }
}
