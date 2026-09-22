//! Runtime façade for authorized agents and explainable claim eligibility (WS-016).
use crate::Runtime;
use awr_core::{
    AgentAuthorization, ClaimEligibilityExplanation, ClaimEvaluationInput,
    DelegateAuthorizationRequest, IssueAuthorizationRequest, PersonId, Result,
    RevokeAuthorizationRequest,
};
use awr_store::agent_authorization::AuthorizationReceipt;

impl Runtime<'_> {
    pub fn get_agent_authorization(
        &self,
        authorization_id: &str,
    ) -> Result<Option<AgentAuthorization>> {
        self.store
            .get_agent_authorization(self.project, authorization_id)
    }

    pub fn list_agent_authorizations(
        &self,
        responsible_person: Option<&PersonId>,
        subject_id: Option<&str>,
        active_only: bool,
    ) -> Result<Vec<AgentAuthorization>> {
        self.store.list_agent_authorizations(
            self.project,
            responsible_person,
            subject_id,
            active_only,
        )
    }

    pub fn issue_agent_authorization(
        &mut self,
        req: &IssueAuthorizationRequest,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        self.store.issue_agent_authorization(self.project, req)
    }

    pub fn revoke_agent_authorization(
        &mut self,
        req: &RevokeAuthorizationRequest,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        self.store.revoke_agent_authorization(self.project, req)
    }

    pub fn delegate_agent_authorization(
        &mut self,
        req: &DelegateAuthorizationRequest,
        now_ms: i64,
    ) -> Result<(AgentAuthorization, AuthorizationReceipt)> {
        self.store
            .delegate_agent_authorization(self.project, req, now_ms)
    }

    pub fn explain_claim(
        &self,
        input: &ClaimEvaluationInput<'_>,
    ) -> Result<ClaimEligibilityExplanation> {
        self.store.explain_claim(input)
    }
}
