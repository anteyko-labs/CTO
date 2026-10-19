// Права сверх роли (ADR-082): владелец выдаёт администратору отдельное право, например перелив.
import type { Permission, User } from './types'

/** Может ли пользователь: владелец — всё, администратор — если владелец выдал право.
 *  Проверяет сервер; здесь — только какие пункты меню показать. */
export function can(user: Pick<User, 'role' | 'permissions'>, permission: Permission): boolean {
  return user.role === 'owner' || (user.permissions ?? []).includes(permission)
}
