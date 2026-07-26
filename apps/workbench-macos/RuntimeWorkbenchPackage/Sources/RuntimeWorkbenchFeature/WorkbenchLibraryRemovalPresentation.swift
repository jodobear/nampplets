enum WorkbenchLibraryRemovalPresentation {
    static func message(for title: String) -> String {
        "\(title) will be removed from your library and any open session will "
            + "stop. Its permissions, saved napplet data, and workspace "
            + "placements will also be removed. Activity history, receipts, "
            + "workspace definitions, and downloaded build files will remain."
    }
}
