//! Runtime façade for task responsibility and execution-instance assignment.
//! Personal mode may default owner=self while retaining the same underlying ops.
use crate::Runtime;
use awr_core::{
    AcceptResponsibilityRequest, AssignResponsibilityRequest, ClaimExecutionRequest,
    ExecutionInstance, PersonAgentBinding, PersonId, ResponsibilityPending, ResponsibilityReceipt,
    Result, TaskResponsibility, TransferOwnerRequest,
};

impl Runtime<'_> {
    pub fn task_responsibility(&self, work_item_id: &str) -> Result<TaskResponsibility> {
        self.store.task_responsibility(self.project, work_item_id)
    }

    pub fn ensure_person(&mut self, person: &PersonId, display_name: &str) -> Result<()> {
        self.store.ensure_person(self.project, person, display_name)
    }

    pub fn bind_person_agent(&mut self, binding: &PersonAgentBinding) -> Result<()> {
        self.store.bind_person_agent(self.project, binding)
    }

    /// Personal mode convenience: seed owner=self without changing assign/accept semantics.
    pub fn ensure_personal_owner(
        &mut self,
        work_item_id: &str,
        self_person: &PersonId,
        request_key: &str,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        let current = self.store.task_responsibility(self.project, work_item_id)?;
        if current.owner.is_some() {
            // Already assigned; return a replay-shaped no-op via assign with same owner.
            let req = AssignResponsibilityRequest {
                request_key: request_key.into(),
                expected_version: current.version,
                owner: current.owner.clone(),
                collaborators: current.collaborators.clone(),
                independent_reviewer: current.independent_reviewer.clone(),
                allow_unassigned: current.owner.is_none(),
                authorized_by: self_person.clone(),
            };
            // If versions match and we would no-op bump, prefer idempotent receipt path:
            return self
                .store
                .assign_responsibility(self.project, work_item_id, &req);
        }
        let req = AssignResponsibilityRequest {
            request_key: request_key.into(),
            expected_version: current.version,
            owner: Some(self_person.clone()),
            collaborators: vec![],
            independent_reviewer: None,
            allow_unassigned: false,
            authorized_by: self_person.clone(),
        };
        let (task, receipt) = self
            .store
            .assign_responsibility(self.project, work_item_id, &req)?;
        let accept = AcceptResponsibilityRequest {
            request_key: format!("{request_key}:accept"),
            expected_version: task.version,
            acceptor: self_person.clone(),
            as_owner: true,
        };
        let (task, _) = self
            .store
            .accept_responsibility(self.project, work_item_id, &accept)?;
        Ok((task, receipt))
    }

    pub fn assign_responsibility(
        &mut self,
        work_item_id: &str,
        req: &AssignResponsibilityRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store
            .assign_responsibility(self.project, work_item_id, req)
    }

    pub fn accept_responsibility(
        &mut self,
        work_item_id: &str,
        req: &AcceptResponsibilityRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store
            .accept_responsibility(self.project, work_item_id, req)
    }

    pub fn claim_execution_responsibility(
        &mut self,
        work_item_id: &str,
        req: &ClaimExecutionRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store
            .claim_execution_responsibility(self.project, work_item_id, req)
    }

    pub fn release_execution_responsibility(
        &mut self,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        by_person: &PersonId,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store.release_execution_responsibility(
            self.project,
            work_item_id,
            request_key,
            expected_version,
            by_person,
        )
    }

    pub fn transfer_owner_responsibility(
        &mut self,
        work_item_id: &str,
        req: &TransferOwnerRequest,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store
            .transfer_owner_responsibility(self.project, work_item_id, req)
    }

    pub fn swap_agent_for_person(
        &mut self,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        person_id: &PersonId,
        new_executor: ExecutionInstance,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store.swap_agent_for_person(
            self.project,
            work_item_id,
            request_key,
            expected_version,
            person_id,
            new_executor,
        )
    }

    pub fn mark_responsibility_pending(
        &mut self,
        work_item_id: &str,
        request_key: &str,
        expected_version: u64,
        pending: ResponsibilityPending,
    ) -> Result<(TaskResponsibility, ResponsibilityReceipt)> {
        self.store.mark_responsibility_pending(
            self.project,
            work_item_id,
            request_key,
            expected_version,
            pending,
        )
    }
}
