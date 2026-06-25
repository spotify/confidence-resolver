use crate::materialization::{MaterializationData, WriteRequest};
use worker::*;

#[durable_object]
pub struct StickyAssignmentDO {
    state: State,
}

impl DurableObject for StickyAssignmentDO {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&self, mut req: Request) -> Result<Response> {
        match req.method() {
            Method::Get => {
                let data: MaterializationData = self
                    .state
                    .storage()
                    .get("data")
                    .await
                    .unwrap_or_default();
                Response::from_json(&data)
            }
            Method::Put => {
                let write_req: WriteRequest = req.json().await?;
                let mut data: MaterializationData = self
                    .state
                    .storage()
                    .get("data")
                    .await
                    .unwrap_or_default();

                for record in &write_req.records {
                    data.merge_record(&record.rule, &record.variant);
                }

                self.state.storage().put("data", &data).await?;
                Response::from_json(&data)
            }
            _ => Response::error("Method not allowed", 405),
        }
    }
}
