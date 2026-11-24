    for hit in hits {
        if !seen.insert((hit.repo_identity.clone(), hit.file.clone())) {
            continue;
        }
        let Some(repo_id) = store.repository_id(&hit.repo_identity).ok().flatten() else {
            continue;
        };
        let Some(root) = store.checkout_root(repo_id).ok().flatten() else {
            continue;
        };
        if let Ok(crate::index::Refresh::Updated | crate::index::Refresh::Deleted) =
            crate::index::refresh_file(store, repo_id, std::path::Path::new(&root), &hit.file)
        {
            changed = true;
        }
    }
