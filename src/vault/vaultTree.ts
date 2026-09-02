import type { FolderEntry, NoteEntry } from "../lib/tauri";

export type VaultTreeRow =
  | {
      kind: "folder";
      relativePath: string;
      label: string;
      depth: number;
    }
  | {
      kind: "note";
      relativePath: string;
      label: string;
      depth: number;
    };

type TreeItem = Omit<VaultTreeRow, "depth">;

function parentPath(relativePath: string): string {
  const separator = relativePath.lastIndexOf("/");
  return separator < 0 ? "" : relativePath.slice(0, separator);
}

function fileName(relativePath: string): string {
  const separator = relativePath.lastIndexOf("/");
  return separator < 0 ? relativePath : relativePath.slice(separator + 1);
}

function compareItems(left: TreeItem, right: TreeItem): number {
  if (left.kind !== right.kind) return left.kind === "folder" ? -1 : 1;
  const labelOrder = left.label.localeCompare(right.label, undefined, {
    sensitivity: "base",
  });
  return labelOrder || left.relativePath.localeCompare(right.relativePath);
}

export function buildVaultTreeRows(
  folders: FolderEntry[],
  notes: NoteEntry[],
): VaultTreeRow[] {
  const folderPaths = new Set<string>();
  const addFolderAndAncestors = (relativePath: string) => {
    let current = relativePath;
    while (current.length > 0) {
      folderPaths.add(current);
      current = parentPath(current);
    }
  };

  for (const folder of folders) addFolderAndAncestors(folder.relativePath);
  for (const note of notes) addFolderAndAncestors(parentPath(note.relativePath));

  const children = new Map<string, TreeItem[]>();
  const addChild = (parent: string, item: TreeItem) => {
    const siblings = children.get(parent) ?? [];
    siblings.push(item);
    children.set(parent, siblings);
  };

  for (const relativePath of folderPaths) {
    addChild(parentPath(relativePath), {
      kind: "folder",
      relativePath,
      label: fileName(relativePath),
    });
  }
  for (const note of notes) {
    addChild(parentPath(note.relativePath), {
      kind: "note",
      relativePath: note.relativePath,
      label: note.title,
    });
  }

  for (const siblings of children.values()) siblings.sort(compareItems);

  const rows: VaultTreeRow[] = [];
  const appendChildren = (parent: string, depth: number) => {
    for (const item of children.get(parent) ?? []) {
      rows.push({ ...item, depth });
      if (item.kind === "folder") {
        appendChildren(item.relativePath, depth + 1);
      }
    }
  };
  appendChildren("", 0);
  return rows;
}
