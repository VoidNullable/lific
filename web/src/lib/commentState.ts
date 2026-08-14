import type { Comment } from "./api";

export type CommentKeyboardContext = "new" | "edit" | "menu";
export type CommentKeyboardAction = "submit" | "save" | "cancel" | "close-menu";

export function commentKeyboardAction(
  context: CommentKeyboardContext,
  key: string,
  modified: boolean,
): CommentKeyboardAction | null {
  if (key === "Enter" && modified) {
    if (context === "new") return "submit";
    if (context === "edit") return "save";
  }
  if (key === "Escape") {
    if (context === "edit") return "cancel";
    if (context === "menu") return "close-menu";
  }
  return null;
}

export function canManageComment(
  comment: Comment,
  currentUser: { id: number } | null,
  actionsAvailable: boolean,
): boolean {
  return actionsAvailable && currentUser?.id === comment.user_id;
}

export function commentWasEdited(comment: Comment): boolean {
  return comment.updated_at !== comment.created_at;
}

export function replaceComment(comments: Comment[], updated: Comment): Comment[] {
  return comments.map((comment) => comment.id === updated.id ? updated : comment);
}

export function removeComment(comments: Comment[], id: number): Comment[] {
  return comments.filter((comment) => comment.id !== id);
}
