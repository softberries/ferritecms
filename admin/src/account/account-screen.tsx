// /admin/account (every role; 07 §3 Account): the signed-in user's profile, password,
// two-factor authentication, API tokens and sessions. A `must_reset` session never gets
// here: the `_app` guard sends it to the forced change first. Created and last sign-in
// times come from `GET /users/{me}` (`MeDto` lacks them).

import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { userQuery } from "@/api/users";
import type { MeDto } from "@/auth/session";
import { ErrorState } from "@/components/kit/error-state";
import { PageHeader } from "@/components/kit/page-header";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { PasswordCard } from "./password-card";
import { ProfileCard } from "./profile-card";
import { SessionsCard } from "./sessions-card";
import { AccountSection } from "./shared";
import { TokensCard } from "./tokens-card";
import { TotpCard } from "./totp-card";

export function AccountScreen({ me }: { me: MeDto }) {
  const { t } = useTranslation("admin");
  const user = useQuery(userQuery(me.id));
  const section = (id: "profile" | "password" | "totp" | "tokens" | "sessions") => ({
    id,
    title: t(`account.sections.${id}.title`),
    description: t(`account.sections.${id}.description`),
  });

  return (
    <div data-slot="account-screen" className="mx-auto flex w-full max-w-5xl flex-col gap-8">
      <PageHeader title={t("account.title")} description={t("account.description")} />
      <div className="flex flex-col gap-8">
        <AccountSection {...section("profile")}>
          {user.data ? (
            <ProfileCard key={user.data.id} user={user.data} />
          ) : user.isError ? (
            <ErrorState error={user.error} retry={() => void user.refetch()} />
          ) : (
            <Card aria-busy="true">
              <CardContent className="flex flex-col gap-5">
                <Skeleton className="h-14 w-full" />
                <div className="grid gap-5 sm:grid-cols-2">
                  <Skeleton className="h-16" />
                  <Skeleton className="h-16" />
                </div>
              </CardContent>
            </Card>
          )}
        </AccountSection>
        <AccountSection {...section("password")}>
          <PasswordCard login={me.login} />
        </AccountSection>
        <AccountSection {...section("totp")}>
          <TotpCard me={me} />
        </AccountSection>
        <AccountSection {...section("tokens")}>
          <TokensCard me={me} />
        </AccountSection>
        <AccountSection {...section("sessions")}>
          <SessionsCard />
        </AccountSection>
      </div>
    </div>
  );
}
