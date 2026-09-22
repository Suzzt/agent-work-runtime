//! Runtime façade for confirmed Team long-term handoffs (WS-017).
use crate::Runtime;
use awr_core::{
    AcceptHandoffRequest, CancelHandoffRequest, HandoffReceipt, InspectHandoffRequest, PersonId,
    ProposeHandoffRequest, RejectHandoffRequest, Result, TeamHandoff, TimeoutHandoffRequest,
};

impl Runtime<'_> {
    pub fn get_team_handoff(&self, handoff_id: &str) -> Result<Option<TeamHandoff>> {
        self.store.get_team_handoff(self.project, handoff_id)
    }

    pub fn propose_team_handoff(
        &mut self,
        work_item_id: &str,
        from_person: &PersonId,
        req: &ProposeHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store
            .propose_team_handoff(self.project, work_item_id, from_person, req)
    }

    pub fn inspect_team_handoff(
        &mut self,
        req: &InspectHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store.inspect_team_handoff(self.project, req)
    }

    pub fn accept_team_handoff(
        &mut self,
        req: &AcceptHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store.accept_team_handoff(self.project, req)
    }

    pub fn reject_team_handoff(
        &mut self,
        req: &RejectHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store.reject_team_handoff(self.project, req)
    }

    pub fn cancel_team_handoff(
        &mut self,
        req: &CancelHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store.cancel_team_handoff(self.project, req)
    }

    pub fn timeout_team_handoff(
        &mut self,
        req: &TimeoutHandoffRequest,
    ) -> Result<(TeamHandoff, HandoffReceipt)> {
        self.store.timeout_team_handoff(self.project, req)
    }
}
