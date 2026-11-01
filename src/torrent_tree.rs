use std::collections::BTreeSet;
use strata::engine::TorrentFile;

pub struct TorrentNode {
    pub id: usize,
    pub name: String,
    pub files: Vec<usize>,
    pub children: Vec<TorrentNode>,
}

impl TorrentNode {
    pub fn find(&self, id: usize) -> Option<&Self> {
        if self.id == id {
            Some(self)
        } else {
            self.children.iter().find_map(|child| child.find(id))
        }
    }
}

pub fn tree(files: &[TorrentFile]) -> (Vec<TorrentNode>, usize) {
    let mut roots = Vec::<TorrentNode>::new();
    let mut next_id = 0;
    for (index, file) in files.iter().enumerate() {
        let mut siblings = &mut roots;
        for name in file.name.split(['/', '\\']).filter(|name| !name.is_empty()) {
            let position = siblings
                .iter()
                .position(|node| node.name == name)
                .unwrap_or_else(|| {
                    let position = siblings.len();
                    siblings.push(TorrentNode {
                        id: next_id,
                        name: name.into(),
                        files: Vec::new(),
                        children: Vec::new(),
                    });
                    next_id += 1;
                    position
                });
            let node = &mut siblings[position];
            node.files.push(index);
            siblings = &mut node.children;
        }
    }
    fn sort(nodes: &mut [TorrentNode]) {
        nodes.sort_by_cached_key(|node| (node.children.is_empty(), node.name.to_lowercase()));
        for node in nodes {
            sort(&mut node.children);
        }
    }
    sort(&mut roots);
    (roots, next_id)
}

pub fn rows<'a>(
    nodes: &'a [TorrentNode],
    expanded: &BTreeSet<usize>,
) -> Vec<(&'a TorrentNode, usize)> {
    fn visit<'a>(
        nodes: &'a [TorrentNode],
        expanded: &BTreeSet<usize>,
        depth: usize,
        rows: &mut Vec<(&'a TorrentNode, usize)>,
    ) {
        for node in nodes {
            rows.push((node, depth));
            if expanded.contains(&node.id) {
                visit(&node.children, expanded, depth + 1, rows);
            }
        }
    }
    let mut rows = Vec::new();
    visit(nodes, expanded, 0, &mut rows);
    rows
}

pub fn toggle(files: &[usize], selected: &mut BTreeSet<usize>) {
    if files.iter().all(|index| selected.contains(index)) {
        for index in files {
            selected.remove(index);
        }
    } else {
        selected.extend(files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_nested_paths_and_toggles_all_descendants_from_partial_selection() {
        let files = [
            "media/Season 1/B.mp4",
            "media/Season 1/A.mp4",
            "media/readme.txt",
            "LICENSE",
        ]
        .map(|name| TorrentFile {
            name: name.into(),
            size: 1,
        });
        let (tree, count) = tree(&files);
        assert_eq!(count, 6);
        let media = &tree[0];
        assert_eq!(media.name, "media");
        assert_eq!(media.files, [0, 1, 2]);
        assert_eq!(media.children[0].name, "Season 1");
        assert_eq!(media.children[0].children[0].name, "A.mp4");
        assert_eq!(media.children[0].children[0].files, [1]);
        assert_eq!(rows(&tree, &BTreeSet::new()).len(), 2);
        let expanded = [media.id].into_iter().collect();
        let visible = rows(&tree, &expanded);
        assert_eq!(
            visible
                .iter()
                .map(|(node, depth)| (node.name.as_str(), *depth))
                .collect::<Vec<_>>(),
            [
                ("media", 0),
                ("Season 1", 1),
                ("readme.txt", 1),
                ("LICENSE", 0)
            ]
        );
        let mut selected = [0].into_iter().collect();
        toggle(&media.files, &mut selected);
        assert_eq!(selected, [0, 1, 2].into_iter().collect());
        toggle(&media.files, &mut selected);
        assert!(selected.is_empty());
        assert_eq!(
            media.find(media.children[0].children[0].id).unwrap().files,
            [1]
        );
    }
}
