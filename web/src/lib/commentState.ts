import type { Comment } from "./api";

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
