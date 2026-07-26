extension CatalogViewModel {
    func beginTransientOperation() -> UInt? {
        guard let generation = operationGeneration.issue() else {
            recordOperationGenerationExhaustion()
            return nil
        }
        return generation
    }

    func recordFeedGenerationExhaustion() {
        guard feedGenerationExhaustion == nil,
              let exhaustion = feedGeneration.exhaustion
        else {
            return
        }
        feedGenerationExhaustion = exhaustion
        feedObservation?.cancel()
        feedObservation = nil
        browseIssue = CatalogIssueNotice.Presentation(
            issue: CatalogIssue(
                title: "Catalog updates stopped",
                message: "Close and reopen the catalog to continue."
            ),
            intent: entries.isEmpty ? .browseBlocked : .browsePartial
        )
    }

    func recordOperationGenerationExhaustion() {
        guard operationGenerationExhaustion == nil,
              let exhaustion = operationGeneration.exhaustion
        else {
            return
        }
        operationGenerationExhaustion = exhaustion
        isResolvingReview = false
        isInstalling = false
        client.cancelPendingCatalogWork()
        operationIssue = CatalogIssueNotice.Presentation(
            issue: CatalogIssue(
                title: "Catalog action stopped",
                message: "Close and reopen the catalog before trying again."
            ),
            intent: .resolveBlocked
        )
    }
}
