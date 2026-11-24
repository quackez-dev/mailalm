# Inscrire MailALM chez Google (Gmail)

MailALM lit les comptes Gmail avec l'API Gmail (PLAN.md, section 4.2). Pour s'y connecter, il lui faut un **projet
Google Cloud** avec un **client OAuth « Desktop app »**, fait une seule fois. Il donne un **ID client** et un
**code secret du client**, que le CI compile dans MailALM.

- Pour une application de bureau, Google ne considère pas ce code secret comme confidentiel : il reste lisible dans
  le binaire. MailALM se connecte avec OAuth 2 et PKCE, comme pour Microsoft.
- Le projet ne donne accès à **aucune boîte**. Chaque personne se connecte ensuite avec son propre compte Gmail et
  accepte elle-même les permissions. Ses jetons restent dans le Gestionnaire d'identifiants de son PC.
- Aucune facturation n'est nécessaire : l'API Gmail est gratuite dans ses quotas.

## Le choix fait : « External », publié sans vérification

Le projet doit servir à **plusieurs comptes Gmail** (décision du 2026-11-24). Google a deux voies, et on ne peut
pas passer de l'une à l'autre :

- **Personal** : un seul compte Google, celui qui crée le projet. Écartée.
- **External**, publié **en production** sans vérification : retenue. Conséquences :
  - l'écran « Google n'a pas validé cette application » s'affiche à la première connexion de chaque compte. On
    clique sur **Paramètres avancés**, puis **Accéder à MailALM (non sécurisé)** ;
  - le projet est limité à **100 nouveaux comptes pour toute sa vie**. La limite ne se remet jamais à zéro ;
  - la vérification par Google n'est pas faite : pour les permissions Gmail (dites « restreintes »), elle exige un
    audit de sécurité payant (CASA).

**Ne laisse pas le projet en mode « Testing »** : les jetons y expirent après 7 jours, et il faudrait se
reconnecter chaque semaine.

## 1. Créer le projet

1. Ouvre `https://console.cloud.google.com` avec le compte Google qui possédera le projet. Un de tes comptes Gmail
   convient ; ce n'est pas forcément un compte que MailALM lira.
2. En haut, le sélecteur de projet → **Nouveau projet**. Nom : `MailALM`. Organisation : **Aucune organisation**.
3. **Créer**, puis vérifie que le projet `MailALM` est sélectionné en haut.

## 2. Activer l'API Gmail

Menu ☰ → **API et services** → **Bibliothèque** → cherche **Gmail API** → **Activer**.

L'agenda, les tâches et les contacts (phase 6) demanderont plus tard Google Calendar API, Google Tasks API et
People API. On les activera à ce moment.

## 3. Configurer l'écran de consentement

Menu ☰ → **Google Auth Platform** (ou **API et services** → **Écran de consentement OAuth**) → **Commencer**
(*Get started*). Choisis la voie normale, **pas** le « Personal mode ».

| Champ | Valeur |
|---|---|
| Nom de l'application | `MailALM` |
| Adresse e-mail d'assistance | ton adresse Gmail (elle s'affiche sur l'écran de connexion) |
| Audience (*Audience*) | **Externe** (*External*) |
| Coordonnées (*Contact information*) | ton adresse Gmail (Google seulement s'en sert pour te prévenir) |

Accepte les conditions, puis **Créer**.

Ensuite, dans **Branding** (*Image de marque*) :

- **n'ajoute pas de logo** : un logo oblige Google à vérifier l'application ;
- laisse vides la page d'accueil, la politique de confidentialité, les conditions et les domaines autorisés.

## 4. Déclarer les permissions

**Accès aux données** (*Data Access*) → **Ajouter ou supprimer des champs d'application** (*Add or remove scopes*).
Colle ces deux adresses dans « Ajouter manuellement », puis **Ajouter au tableau** → **Mettre à jour** →
**Enregistrer** :

| Permission | Pourquoi |
|---|---|
| `https://www.googleapis.com/auth/gmail.modify` | Lire, envoyer, marquer, déplacer (libellés), mettre à la corbeille, signaler comme spam |
| `https://www.googleapis.com/auth/gmail.settings.basic` | Filtres (bloquer ou autoriser un expéditeur, règles) et réponse automatique |

C'est le minimum. Il manque volontairement l'accès complet (`https://mail.google.com/`) : MailALM ne pourra donc pas
**effacer définitivement** un message. Un message supprimé va à la Corbeille de Gmail, que Gmail vide lui-même après
30 jours.

Google classe ces deux permissions comme « restreintes » et affichera un avertissement sur la vérification.
**Ne soumets rien à la vérification.**

## 5. Publier en production

**Audience** → **État de publication : Test** → **Publier l'application** (*Publish app*) → **Confirmer**.

L'état doit devenir **En production**. La liste des « utilisateurs test » ne sert plus.

## 6. Créer le client OAuth

**Clients** → **Créer un client** (*Create client*) :

| Champ | Valeur |
|---|---|
| Type d'application | **Application de bureau** (*Desktop app*) |
| Nom | `MailALM` |

**Créer**. Google affiche alors l'**ID client** (qui finit par `.apps.googleusercontent.com`) et le **code secret
du client** (qui commence par `GOCSPX-`).

**Le code secret ne s'affiche qu'une fois.** Pose-le tout de suite dans GitHub (étape 7), avant de fermer la
fenêtre. Ne télécharge pas le fichier JSON et ne le colle nulle part ailleurs. Si tu le perds, le client a un bouton
**Ajouter un code secret** (*Add secret*) pour en créer un nouveau.

Aucune adresse de redirection n'est à déclarer : pour un client de bureau, Google accepte toute adresse locale
`http://127.0.0.1:<port>`. MailALM ouvre un port libre au moment de la connexion, sur ton PC seulement.

## 7. Donner les identifiants au CI

Ils restent hors du code public (décision Q5). Deux secrets sur le dépôt `quackez-dev/mailalm` :

| Nom | Valeur |
|---|---|
| `MAILALM_GOOGLE_CLIENT_ID` | l'ID client |
| `MAILALM_GOOGLE_CLIENT_SECRET` | le code secret du client |

- **Par le site** : **Settings** → **Secrets and variables** → **Actions** → **New repository secret**, une fois
  par secret.
- **Ou en ligne de commande**, une commande par secret. Chacune demande la valeur sans l'afficher, et sans la laisser
  dans l'historique :

  ```
  gh secret set MAILALM_GOOGLE_CLIENT_ID --repo quackez-dev/mailalm
  gh secret set MAILALM_GOOGLE_CLIENT_SECRET --repo quackez-dev/mailalm
  ```

Le workflow ne donne jamais ces secrets à une exécution venue d'une pull request.

## À savoir

- Google **efface un client OAuth inactif depuis six mois**. Si MailALM ne se connecte à aucun compte Gmail pendant
  six mois, il faudra refaire l'étape 6 et changer les deux secrets.
- Si un jour les connexions Gmail expirent chaque semaine, vérifie d'abord que l'**Audience** est toujours
  **En production**.

## Ce que MailALM contactera

`accounts.google.com` pour la connexion, `oauth2.googleapis.com` pour les jetons, et `gmail.googleapis.com` pour le
courriel. `docs/domaines.md` sera complété en phase 4.
