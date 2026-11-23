# Inscrire MailALM chez Microsoft (Hotmail, Outlook.com)

MailALM lit les comptes Hotmail et Outlook.com avec Microsoft Graph (PLAN.md, section 4.2). Pour s'y connecter, il
lui faut une **inscription d'application** chez Microsoft, faite une seule fois. Elle donne un identifiant public,
l'**ID d'application (client)**, que le CI compile dans MailALM.

- Aucun mot de passe d'application ni « secret client » : MailALM est une application de bureau publique et se
  connecte avec OAuth 2 et PKCE.
- L'inscription ne donne accès à **aucune boîte**. Chaque personne se connecte ensuite avec son propre compte et
  accepte elle-même les permissions. Ses jetons restent dans le Gestionnaire d'identifiants de son PC.

## 1. Avoir un annuaire Microsoft Entra

Depuis juin 2024, toute inscription doit être faite dans un annuaire (tenant) Entra. Un compte Microsoft personnel
seul n'en a pas.

- **Recommandé** : créer un compte Azure gratuit avec un compte Microsoft personnel (par exemple ton Hotmail), sur
  `https://azure.microsoft.com/free`. Azure crée alors un annuaire « Default Directory ».
  - Microsoft demande une carte de crédit pour vérifier l'identité. L'essai ne se transforme pas en abonnement
    payant sans ton accord explicite.
  - Les inscriptions d'applications font partie d'Entra ID Free et ne coûtent rien.
  - Point non garanti par Microsoft : que tout continue de marcher sans abonnement Azure actif après l'essai. Si une
    connexion échoue un jour pour cette raison, on verra à ce moment.
- **À éviter** : l'annuaire d'un employeur. L'application lui appartiendrait, et ses administrateurs pourraient la
  modifier ou la supprimer.

## 2. Créer l'inscription

1. Ouvre `https://entra.microsoft.com` et connecte-toi avec le compte qui possède l'annuaire.
2. Va dans **Entra ID** → **Inscriptions d'applications** (*App registrations*) → **Nouvelle inscription**.
3. Remplis :

   | Champ | Valeur |
   |---|---|
   | Nom | `MailALM` |
   | Types de comptes pris en charge | **Comptes Microsoft personnels uniquement** (*Personal Microsoft accounts only*) |
   | URI de redirection : plateforme | **Client public/natif (mobile et bureau)** (*Public client/native*) |
   | URI de redirection : valeur | `http://localhost` |

4. Clique sur **Inscrire**.

`http://localhost` sans port est voulu : pour une application de bureau, Microsoft accepte n'importe quel port local.
MailALM ouvre un port libre au moment de la connexion, sur ton PC seulement.

## 3. Vérifier les réglages

- **Vue d'ensemble** : note l'**ID d'application (client)**, un GUID comme `00000000-0000-0000-0000-000000000000`.
  L'ID d'annuaire n'est pas utile : MailALM passe par le point d'entrée des comptes personnels (`consumers`).
- **Authentification** :
  - l'URI `http://localhost` apparaît sous « Applications mobiles et de bureau » ;
  - « Autoriser les flux de clients publics » reste à **Non**. MailALM n'utilise ni le code d'appareil ni le mot de
    passe direct.
- **Certificats et secrets** : n'en crée **aucun**.

## 4. Déclarer les permissions

Dans **Autorisations de l'API** (*API permissions*) → **Ajouter une autorisation** → **Microsoft Graph** →
**Autorisations déléguées**, ajoute :

| Permission | Pourquoi |
|---|---|
| `offline_access` | Rester connecté sans redemander le mot de passe à chaque démarrage |
| `User.Read` | Lire le nom et l'adresse du compte |
| `Mail.ReadWrite` | Lire, déplacer, marquer et supprimer les messages ; bloquer un expéditeur (`markAsJunk`) |
| `Mail.Send` | Envoyer |
| `MailboxSettings.ReadWrite` | Règles de boîte de réception, réponse automatique, catégories |

Pour un compte personnel, aucun « consentement administrateur » n'est nécessaire. Chaque personne accepte les
permissions à sa première connexion. L'écran indiquera « éditeur non vérifié » : c'est normal pour une application
privée.

L'agenda, les contacts et les tâches (phase 6) ajouteront plus tard `Calendars.ReadWrite`, `Contacts.ReadWrite` et
`Tasks.ReadWrite`. MailALM les demandera à ce moment, avec un nouvel écran de consentement.

## 5. Donner l'identifiant au CI

L'ID n'est pas un secret au sens strict : il est lisible dans le binaire. Il reste hors du code public (décision
Q5).

- **Par le site** : dépôt `quackez-dev/mailalm` → **Settings** → **Secrets and variables** → **Actions** → **New
  repository secret**. Nom : `MAILALM_MICROSOFT_CLIENT_ID`. Valeur : l'ID d'application.
- **Ou en ligne de commande** : `gh secret set MAILALM_MICROSOFT_CLIENT_ID --repo quackez-dev/mailalm`. La commande
  demande la valeur sans l'afficher, et sans la laisser dans l'historique.

Le workflow ne donne jamais ce secret à une exécution venue d'une pull request.

## Ce que MailALM contactera

`login.microsoftonline.com` pour la connexion et `graph.microsoft.com` pour le courriel. `docs/domaines.md` sera
complété en phase 3.
