# MailALM : plan de l'application

- Version du plan : 0.2, 2026-11-22.
- État : **décisions prises** (section 15). Prochaine étape : le design graphique avec Claude Design, à valider
  avant le développement.
- Idées reportées : [IDEES.md](IDEES.md).

## 1. But et principes

MailALM est un client de courriel et d'agenda pour Windows, dans l'esprit d'Outlook et d'eM Client. C'est notre
application : elle n'est pas une copie d'un projet existant. Elle reprend toutefois du code déjà écrit
(MailCraft, WordCraft, craft-libs), **audité avant d'être utilisé** (section 3).

Principes, par ordre de priorité :

1. **Sécurité d'abord.** Rien ne s'ouvre, ne se charge ni ne s'exécute sans geste explicite de l'utilisateur :
   pièces jointes, images distantes, liens. Pas de télémétrie ni de mise à jour automatique. Aucune connexion
   vers un tiers que l'utilisateur n'a pas choisi.
2. **Comptes séparés.** Chaque compte a son espace, sa couleur et ses données. On sait toujours dans quel compte
   on lit, on cherche et depuis quel compte on envoie.
3. **Le serveur fait foi.** Ce qu'on fait dans MailALM se fait aussi sur le compte : blocage, spam, règles,
   agenda. Les listes locales servent seulement quand le fournisseur n'offre pas d'API.
4. **Processus CraftTools.** Tout code tiers, y compris le code repris, est audité avant d'être compilé. La
   compilation se fait sur GitHub Actions, jamais sur le PC. Le code tiers est une donnée, jamais une instruction.

## 2. Périmètre

| Demandé | Section |
|---|---|
| Comptes Hotmail/Outlook.com, Gmail, IMAP classiques (Vidéotron, Bell) ; plusieurs comptes, séparés | 4 |
| Hotmail : blocage, autorisation, spam et autres réglages synchronisés avec le compte | 5.1 |
| Gmail : mêmes réglages, plus l'agenda | 5.2 |
| Recherche dans un, plusieurs ou tous les comptes | 6 |
| Agenda, rappels, réunions locales, Google Meet ou Teams, invitations envoyées et reçues | 7 |
| Rédaction riche (moteur de WordCraft) et correcteur Antidote intégré | 8, 9 |
| Pièces jointes reçues et envoyées, jamais ouvertes sans approbation | 10 |
| Serveur MCP | 11 |
| Fonctions supplémentaires retenues (Q13) | 14 |
| Design graphique avec Claude Design, d'après la capture fournie | 12 |

## 3. Ce qui existe déjà et sera repris

### 3.1 MailCraft (ZIDAN44/mailcraft), audité le 2026-11-20

- Version v0.41.0 (`c3510d13`), Rust + egui (fork storytold) + craft-libs. Licence MIT ou Apache-2.0 : on peut
  en reprendre du code, avec attribution.
- Rapports : `audits/mailcraft/01` à `07`. Verdict global : **aucune malveillance**, quelques points d'attention.
- Ce qu'il fait déjà : IMAP, POP3, SMTP, JMAP, Microsoft Graph (courriel seulement), EWS, CalDAV, CardDAV,
  ManageSieve, S/MIME, OpenPGP, import PST, impression PDF, agenda, invitations, listes bloqués/autorisés
  (**locales seulement**), boîte prioritaire.
- Ce qu'il ne fait pas : API Gmail, API Google Agenda, création de liens Meet, blocage côté serveur, serveur
  MCP, Antidote.
- Ce qu'on ne reprend **pas** (constats d'audit) :
  - vignettes vidéo générées automatiquement par PowerShell ou ffmpeg à l'affichage du message ;
  - ouverture des pièces jointes par liste de refus d'extensions ;
  - autoconfiguration qui interroge Mozilla (ISPDB) et découverte CalDAV/CardDAV automatique ;
  - module de mise à jour intégré ;
  - clés privées gardées déverrouillées.

### 3.2 WordCraft (storytold), audité

- `doc` (modèle de document), `layout` (mise en forme, coupure de lignes, tableaux), `formats` (import et export
  HTML, Markdown, RTF, texte), `engine` (sélection, annulation, commandes).
- `mcp` : serveur MCP sur stdio, relié à l'application par un canal de contrôle protégé par une clé
  (`control-key`). C'est le modèle à suivre.

### 3.3 craft-libs (storytold), audité

`proof` (correcteur intégré), `i18n`, `dialogs`, `settings`, `menu`, `keymap`, `fonts`, `print`, `pdf`,
`platform`.

### 3.4 Ce qui est repris (décision Q1)

Une **nouvelle application**, avec sa propre architecture, sa propre interface et son propre moteur, qui
**reprend du code audité** là où réécrire n'apporte rien :

| Brique | Origine |
|---|---|
| Analyse et construction MIME, fils de discussion, modèle de données | MailCraft `mime`, `model` (copie à v0.41.0, attribution) |
| IMAP, SMTP, OAuth (PKCE), Microsoft Graph courriel | MailCraft `net`, relu et réduit (sans autoconfig vers des tiers, sans découverte DAV automatique, sans updater) |
| API Gmail, Google Agenda, Meet, Teams, Graph agenda, tâches, contacts, `markAsJunk` | **nouveau** |
| Cache local | **nouveau**, inspiré de MailCraft `store` |
| Moteur de rédaction | WordCraft `doc`, `layout`, `formats` (export HTML) |
| Correcteur | Antidote (section 9), craft-proof en secours |
| Interface | **nouvelle**, d'après le design validé |
| MCP | **nouveau**, sur le modèle de WordCraft `mcp` et `control-key` |
| Pièces jointes, rendu HTML, sécurité | **nouveau** (section 10) |

Règles de reprise :

- Le code repris est **copié** dans le dépôt de MailALM, pas lié par une dépendance vivante. Il devient le
  nôtre. Chaque fichier garde la mention de son origine (dépôt, commit, chemin) et `ATTRIBUTION.md` liste tout.
- **Audit avant usage.** Les audits de MailCraft et de WordCraft portaient sur leur usage dans ces applications.
  Avant d'intégrer un fichier, on le **relit en entier** dans son nouvel usage : entrées non fiables (courriels,
  pièces jointes, réponses de serveur), plafonds de taille, erreurs, chemins de fichiers, réseau, `unsafe`. On
  retire ce dont MailALM n'a pas besoin. Rapport dans `audits/mailalm/reprise-<brique>.md`, avec le SHA d'origine
  et la liste des changements.
- Les dépendances storytold (egui, winit, craft-libs) restent épinglées aux révisions déjà auditées et sont
  récupérées depuis les copies quackez-dev. Toute nouvelle crate crates.io est auditée comme pour les autres
  applications (`audits/mailalm/`).

## 4. Comptes

### 4.1 Types de comptes

| Fournisseur | Domaines reconnus | Courriel | Agenda | Tâches | Contacts | Authentification |
|---|---|---|---|---|---|---|
| Hotmail / Outlook.com | `@hotmail.com`, `@outlook.com`, `@live.ca` (et variantes Microsoft) | Microsoft Graph | Graph | Microsoft To Do (Graph) | Graph | OAuth 2 (PKCE) |
| Gmail | `@gmail.com` | API Gmail | API Google Agenda | Google Tasks | API Google People | OAuth 2 (PKCE) |
| Vidéotron | `@videotron.ca` | IMAP + SMTP | agenda local (CalDAV si offert) | locales | locaux (CardDAV si offert) | mot de passe (trousseau Windows) |
| Bell | `@bell.net` | IMAP + SMTP | agenda local (CalDAV si offert) | locales | locaux (CardDAV si offert) | mot de passe (trousseau Windows) |
| Autre fournisseur | saisie manuelle | IMAP + SMTP | CalDAV ou local | locales | CardDAV ou locaux | mot de passe |

- Microsoft n'accepte plus le mot de passe seul pour IMAP sur Outlook.com : Graph et OAuth sont la seule voie
  pour Hotmail. Graph donne en plus le blocage côté serveur, les règles, les catégories et l'agenda.
- Gmail fonctionne aussi en IMAP avec un mot de passe d'application, mais sans filtres, sans agenda ni Meet. On
  utilise l'API Gmail.
- Profils intégrés pour Vidéotron et Bell (serveurs, ports, TLS), vérifiés avec l'utilisateur. Pas
  d'autoconfiguration par un service tiers.

### 4.2 Identifiants OAuth (décision Q5)

- **Une seule** application Azure (« comptes Microsoft personnels ») et **un seul** projet Google Cloud, créés
  par l'utilisateur avec un guide pas à pas (`docs/oauth-microsoft.md`, `docs/oauth-google.md`). Ils servent pour
  tous les PC et tous les utilisateurs : chacun se connecte ensuite avec son propre compte, et ses jetons restent
  dans le trousseau Windows de son PC.
- Les identifiants sont **compilés par le CI** depuis des secrets GitHub (`MAILALM_MICROSOFT_CLIENT_ID`,
  `MAILALM_GOOGLE_CLIENT_ID`, `MAILALM_GOOGLE_CLIENT_SECRET`), posés par l'utilisateur. Ils ne sont pas dans le code
  public. Ils restent extractibles du binaire. C'est accepté : pour une application de bureau, Google ne les
  considère pas comme confidentiels, et le seul risque est l'usage du quota par un tiers.
- Le workflow ne donne jamais ces secrets à une exécution venue d'une pull request.
- Google : tant que l'application n'est pas vérifiée par Google, l'écran « application non validée » s'affiche à
  la première connexion, et le nombre d'utilisateurs est limité (environ 100, à confirmer à la mise en place). Le
  projet doit être en mode « production » : en mode « test », les jetons expirent après 7 jours.

### 4.3 Séparation des comptes (décision Q10)

- Chaque compte a :
  - une **couleur** et un nom court, visibles partout : arborescence, liste, volet de lecture, fenêtre de
    rédaction, agenda, notifications ;
  - ses **données** à part : un dossier et une base par compte. Supprimer un compte efface tout ce qui le
    concerne ;
  - son **worker réseau** : une panne ou un blocage sur un compte n'affecte pas les autres ;
  - ses réglages : signature, modèles, réponses automatiques, listes, règles, agendas.
- Le volet de gauche montre les comptes l'un sous l'autre, **en accordéon** (décision du 2026-11-22) : le compte qui a
  le focus est toujours déplié, les autres sont repliés. Un clic sur un compte lui donne le focus, le déplie, ouvre sa
  boîte de réception et replie les autres. Un clic sur la vue unifiée replie tous les comptes.
- **Le compte qui a le focus est le compte de travail** : son nom est écrit en petit sous « New mail », et le bouton
  ouvre un courriel depuis ce compte. La recherche porte aussi par défaut sur lui.
- **Vue unifiée** : le bouton n'affiche aucun nom de compte, et un clic ouvre le choix du compte (comme la flèche).
  Aucun compte par défaut : l'utilisateur choisit toujours l'expéditeur. La recherche porte sur tous les comptes.
- **Un seul compte configuré** : pas de vue unifiée ni de « Flagged, all accounts » ; ce compte a toujours le focus.
- La flèche de New mail ouvre : « New mail from » (les comptes, l'expéditeur courant coché), « New mail from a
  template… », puis New event, New meeting, New task, New contact. Dans l'agenda et les tâches, le bouton principal
  devient New event ou New task, avec le même menu.
- **Vue unifiée en option** : une section « Favoris » réunit les boîtes de réception, chaque message avec la
  pastille de couleur de son compte. Elle peut être désactivée.
- À l'envoi, l'expéditeur est toujours affiché en clair, avec sa couleur. Une réponse part du compte qui a reçu le
  message. Changer d'expéditeur demande une confirmation si le destinataire n'a jamais écrit à ce compte.

## 5. Fonctions propres à chaque fournisseur

### 5.1 Hotmail / Outlook.com (Microsoft Graph)

| Fonction | Côté serveur | Comment |
|---|---|---|
| Bloquer un expéditeur | **oui** | `message: markAsJunk` (ajoute l'expéditeur aux expéditeurs bloqués du compte et déplace le message dans Courrier indésirable) |
| Débloquer, « pas un courrier indésirable » | **oui** | `message: markAsNotJunk` (retire l'expéditeur de la liste et ramène le message) |
| Lire la liste des bloqués, gérer les expéditeurs approuvés | **non** : aucune API publique | MailALM garde le miroir de ce qu'il a bloqué. Les approuvés restent gérés dans Outlook.com ; MailALM renvoie à la page des réglages |
| Règles de boîte de réception | oui | `messageRules` (à confirmer pour les comptes personnels en phase 3) |
| Réponse automatique (absence) | oui | `mailboxSettings.automaticReplies` |
| Catégories, drapeaux, épinglage | oui | `outlookCategory`, `flag` |
| Boîte prioritaire | oui | `inferenceClassification` |
| Agenda, invitations, rappels | oui | Graph `calendar`, `events` |
| Réunion Teams | à vérifier | `isOnlineMeeting` sur l'événement, si un compte personnel le permet (phase 6) |
| Tâches | oui | Microsoft To Do (`todo/lists`) |
| Contacts | oui | Graph `contacts` |

### 5.2 Gmail (API Gmail, Google Agenda)

| Fonction | Côté serveur | Comment |
|---|---|---|
| Signaler comme spam | oui | libellé `SPAM` sur le message (`messages.modify`) |
| Bloquer un expéditeur | **oui, par filtre** | filtre `from:` → **Spam** si Gmail l'accepte (non documenté, testé en phase 4), **sinon Corbeille** (`addLabelIds: TRASH`, documenté). La liste « Adresses bloquées » de l'interface Gmail n'a pas d'API (décision Q6) |
| Autoriser un expéditeur | oui, par filtre | filtre `from:` → « Ne jamais envoyer dans le spam » (`removeLabelIds: SPAM`) |
| Filtres, libellés | oui | `settings.filters`, `labels` |
| Réponse automatique | oui | `settings.vacation` |
| Boîte prioritaire | oui | catégories Gmail (Principale, Promotions, Réseaux sociaux…) |
| Agenda, invitations, rappels | oui | API Google Agenda |
| Réunion Google Meet | oui | création d'événement avec `conferenceData.createRequest` (`hangoutsMeet`) |
| Tâches | oui | API Google Tasks |
| Contacts | oui | API People |

Les filtres créés par MailALM ont une forme reconnaissable et sont listés dans MailALM : on peut les retirer d'un
clic.

### 5.3 IMAP classique (Vidéotron, Bell, autres)

- Le blocage et l'autorisation sont **locaux** : l'expéditeur bloqué va dans le dossier Indésirables du serveur
  à la réception, quand MailALM tourne.
- Si le serveur offre ManageSieve, le blocage peut devenir une règle Sieve côté serveur (à vérifier pour
  Vidéotron et Bell).
- Signaler un spam : déplacement dans le dossier Indésirables du serveur.

## 6. Recherche

- **Portée choisie à chaque recherche** : compte courant, sélection de comptes (cases à cocher), tous les comptes.
  Puis dossier courant, sous-dossiers, ou tout le compte.
- Opérateurs : `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `is:flagged`, `before:`, `after:`,
  `in:`, `account:`, `category:`.
- Index local plein texte, par compte : instantané, hors ligne. Une recherche côté serveur complète si besoin
  (Graph `$search`, Gmail `q=`, IMAP `SEARCH`), avec la mention « résultats du serveur ».
- Les résultats montrent la couleur du compte. Les recherches peuvent être enregistrées (dossiers de recherche).

## 7. Agenda, rappels, réunions et tâches

- Vues jour, semaine de travail, semaine, mois, liste (agenda). Plusieurs agendas par compte, affichés ensemble
  ou séparément, aux couleurs de leur compte.
- Événements : récurrence, fuseaux horaires, lieu, description riche, pièces jointes, disponibilité (occupé,
  libre, provisoire, absent).
- **Rappels** comme eM Client : fenêtre de rappels avec « Reporter » (5 min à 1 semaine) et « Ignorer »,
  notification Windows, rappels par défaut par agenda. Ils fonctionnent hors ligne. Ils ne sonnent que si
  MailALM tourne, avec une icône de zone de notification en option.
- **Disponibilités globales** (A3) : une vue qui superpose tous les agendas de tous les comptes, pour voir et
  éviter les conflits entre comptes. Elle sert aussi à la planification et à l'analyse des invitations.
- **Planifier une réunion** :
  - locale : lieu, salle ;
  - virtuelle :
    - **Google Meet**, créé par l'API Google Agenda (compte Gmail) ;
    - **Teams** (A2), créé par Graph depuis un compte Hotmail, si l'API le permet pour un compte personnel ;
    - ou un lien libre ;
  - invités, avec leurs disponibilités quand le fournisseur les donne ;
  - **envoi des invitations** : par l'agenda du fournisseur (Graph, Google) ou en iMIP (fichier `.ics`, RFC 5546
    et 6047) pour un compte IMAP.
- **Invitations reçues** :
  - affichées dans le message : date, heure, lieu, lien, organisateur, **conflits** avec tous les agendas ;
  - « Accepter », « Provisoire », « Refuser », « Proposer une autre heure », avec ou sans réponse à
    l'organisateur ;
  - **l'événement n'entre dans l'agenda qu'à la confirmation** (MailCraft, lui, l'ajoute d'office comme
    provisoire) ;
  - mises à jour et annulations appliquées à l'événement déjà accepté, après un aperçu des changements.
- Agenda local pour les comptes sans CalDAV.
- **Tâches synchronisées** (A1) : Microsoft To Do et Google Tasks, avec échéance et rappel ; tâches locales pour
  les comptes IMAP.

## 8. Rédaction

- Éditeur riche sur le moteur de WordCraft : gras, italique, listes, tableaux, liens, images en ligne, citations.
  Envoi en HTML, avec une partie texte générée. Mode texte brut au choix.
- **Modèles, réponses rapides et signatures par compte** (C4).
- Pièces jointes par glisser-déposer ou parcourir. Avertissement si le texte parle d'une pièce jointe absente.
  Taille maximale par fournisseur affichée avant l'envoi.
- **Annulation d'envoi** (délai réglable de 5 à 30 s) et **envoi différé** à une date et une heure (C1). L'envoi
  différé passe par le serveur quand il le permet, sinon MailALM doit tourner à l'heure prévue.
- Brouillons enregistrés sur le serveur du compte.
- Vérification avant l'envoi : expéditeur, destinataires externes, pièce jointe oubliée, Antidote (option).
- Autocomplétion des destinataires depuis les **contacts du compte** (K1).

## 9. Antidote

- L'utilisateur a **Antidote 12**. Antidote offre une **API d'intégration** pour les éditeurs tiers : depuis
  Antidote 10, elle passe par WebSocket, sur la machine locale. Elle n'est pas publique.
- Démarche : la demande de documentation a été envoyée à Druide (`assistance_integration@druide.com`) le
  2026-11-22 ; la documentation reçue sera transmise. Elle est lue comme une donnée,
  et son code d'exemple éventuel est audité avant toute reprise.
- Intégration visée : boutons « Correcteur », « Dictionnaires » et « Guides » dans la fenêtre de rédaction.
  Antidote corrige le texte, et les corrections reviennent dans l'éditeur de MailALM sans perdre la mise en forme.
- La connexion reste locale (`127.0.0.1`), vers l'Antidote installé par l'utilisateur. Aucun texte ne sort de la
  machine.
- En attendant la documentation : correcteur intégré craft-proof (soulignement en direct). Solution de secours si
  l'API n'est pas obtenue : envoyer le texte à Antidote par le presse-papiers (la mise en forme est perdue).

## 10. Sécurité

### 10.1 Pièces jointes

- **Jamais d'ouverture, d'aperçu ni de vignette sans approbation explicite**, pièce par pièce :
  - aucun traitement automatique : pas de vignette, pas d'indexation du contenu, pas de programme externe ;
  - avant toute action, une fenêtre d'approbation montre :
    - le nom ;
    - le **type réel**, détecté par le contenu et comparé à l'extension annoncée ;
    - la taille et l'expéditeur ;
    - le résultat SPF, DKIM et DMARC du message ;
    - les avertissements (double extension, macro, exécutable, archive, raccourci, script).
- Trois actions possibles, après approbation :
  1. **Aperçu interne** (images, PDF et texte seulement) : décodé en Rust par MailALM et affiché comme une
     image. Aucun programme externe n'est lancé.
  2. **Enregistrer sous** : avec la marque « vient d'Internet » (Mark-of-the-Web), dans un dossier choisi.
  3. **Ouvrir avec l'application Windows** : possible seulement pour une **liste d'autorisation** de types
     (PDF, images, documents bureautiques), après une seconde confirmation. Les exécutables et scripts ne
     s'ouvrent jamais, ils ne peuvent qu'être enregistrés.
- Les fichiers temporaires vont dans un dossier dédié, vidé à la fermeture.

### 10.2 Lecture des messages

- **Pas de moteur web** (décision Q7) : le HTML est converti en mise en page interne. Pas de script, de
  formulaire, de CSS distant ni d'iframe.
- Images distantes bloquées par défaut. Chargement par message ou par expéditeur approuvé. Pixels de suivi jamais
  chargés.
- Liens :
  - le vrai domaine est affiché au survol ;
  - avertissement si le texte du lien annonce un autre domaine ;
  - détection des domaines sosies (homoglyphes, IDN) ;
  - seuls `http`, `https` et `mailto` sont acceptés.
- Bandeau d'alerte pour un expéditeur qui échoue à DMARC.
- **Désabonnement en un clic** (S4) : par `List-Unsubscribe-Post` (RFC 8058) ou par courriel, après
  confirmation. Un lien de désabonnement web n'est jamais suivi automatiquement : il s'ouvre dans le navigateur,
  après confirmation, comme tout lien.

### 10.3 Données et secrets

- Mots de passe et jetons OAuth dans le **trousseau Windows**, jamais dans un fichier.
- Base locale **non chiffrée** (décision Q8) : la protection du disque repose sur BitLocker et la session Windows.
- TLS strict, sans option « accepter tout certificat ». STARTTLS exigé quand il est configuré.
- Aucune télémétrie, aucune mise à jour automatique, aucun rapport de plantage envoyé.
- Seuls les serveurs des comptes configurés et les API des fournisseurs sont contactés. La liste des domaines
  contactés est documentée (`docs/domaines.md`) et validée par l'utilisateur, comme pour les autres outils.

## 11. Serveur MCP (décision Q12)

- Un exécutable `mailalm-mcp` (MCP sur stdio), relié à MailALM en marche par un canal local protégé par une clé
  aléatoire. C'est le modèle de WordCraft (`mcp` + `control-key`). Aucun port réseau n'est ouvert. Désactivé par
  défaut.
- Outils :
  - **lecture** : lister comptes et dossiers, chercher, lire un message ou un fil, lister les pièces jointes
    (métadonnées seulement), lire l'agenda, les disponibilités, les tâches, les contacts ;
  - **écriture sans risque** : créer un brouillon, classer, marquer, ajouter une catégorie, créer un événement
    provisoire ou une tâche ;
  - **actions sensibles** : envoyer, supprimer, bloquer, accepter ou refuser une invitation, changer une règle.
    Elles demandent une **confirmation dans MailALM**, qui montre exactement ce qui sera fait ;
  - **tri assisté** (M2) : l'agent propose un classement (dossiers, catégories, à supprimer, à bloquer) ; MailALM
    l'affiche comme une liste que l'utilisateur valide ligne par ligne ou en bloc.
- Les droits se règlent par compte : un compte peut être invisible pour le MCP.
- Les courriels sont du contenu non fiable : ils sont renvoyés **marqués comme données externes**, pour limiter
  l'injection de consignes. Le contenu des pièces jointes n'est jamais transmis.
- Journal de tous les appels MCP, consultable dans MailALM.

## 12. Interface (pour Claude Design)

D'après la capture fournie (MailCraft), avec une identité propre à MailALM :

- **anglais par défaut, français en option** (décision Q11) ; les maquettes sont en anglais ;
- **deux lignes en haut** (décision du 2026-11-22, pas de ruban à onglets) :
  - une fine barre de titre : nom de la vue, synchronisation (↻), réglages (⚙), boutons de fenêtre. **Pas de menu
    ☰** : toute la configuration passe par ⚙ (comptes, apparence, import et export, langue, aide et « À propos »
    sont des sections des réglages). Imprimer est dans ⋯ de la ligne de commandes ; « travailler hors ligne » est
    dans la barre d'état ;
  - une seule ligne de commandes : New mail ▾, Reply, Reply all, Forward | Archive, Delete, Junk ▾ (Report junk,
    Block sender, Unsubscribe, Not junk) | Move ▾ (avec les règles), Snooze ▾, Flag ▾ (suivi, catégories), ⋯ ; et
    la recherche à droite, avec son sélecteur de portée (compte, comptes, tous) ;
- volet de gauche : Favorites (vue unifiée, optionnelle), puis chaque compte avec sa couleur, ses dossiers, ses
  dossiers de recherche ;
- liste des messages groupée par date (Today, Yesterday, Last week…), avec onglets Focused/Other et filtres All,
  Unread, Flagged, Has files ;
- volet de lecture : en-tête, bandeau de sécurité, pièces jointes **verrouillées** (cadenas jusqu'à approbation) ;
- barre de navigation en bas à gauche : Mail, Calendar, Contacts, Tasks ;
- fenêtres (décision du 2026-11-22) :
  - les **réglages** sont une page de la fenêtre principale (même barre de titre, « ← Back to Mail ») ;
  - la **rédaction** et la **nouvelle réunion** s'ouvrent dans des **fenêtres séparées**, avec leurs propres
    boutons réduire, agrandir et fermer (lire un courriel en écrivant, plusieurs brouillons côte à côte) ;
  - « Ajouter un compte » et « Approuver une pièce jointe » sont des dialogues par-dessus la fenêtre ;
- thèmes sombre et clair.

Écrans à concevoir :

1. la fenêtre principale (courrier), avec la vue unifiée et un compte déplié ;
2. l'agenda (vue semaine), avec la superposition des disponibilités de tous les comptes ;
3. la rédaction : barre Antidote, modèles, signature, annulation d'envoi et envoi différé ;
4. la fenêtre d'approbation d'une pièce jointe ;
5. une invitation reçue, avec les conflits ;
6. la planification d'une réunion (Meet ou Teams) ;
7. l'ajout d'un compte ;
8. la gestion des expéditeurs bloqués et autorisés, et l'éditeur de règles (local et serveur) ;
9. les réglages MCP, leur journal et une proposition de tri assisté à valider ;
10. les tâches.

## 13. Architecture technique

- Rust, egui et winit (forks storytold audités), craft-libs.
- Espace de travail Cargo :

```
mailalm/
├── apps/mailalm/          ← application de bureau
├── apps/mailalm-mcp/      ← serveur MCP (stdio)
├── crates/model/          ← comptes, dossiers, messages, événements, tâches, contacts, requêtes (pur)
├── crates/mime/           ← MIME, iCalendar (iTIP/iMIP)
├── crates/store/          ← cache local, index de recherche, file hors ligne
├── crates/net/            ← workers par compte : imap, smtp, graph, gmail, gcal, gtasks, people, caldav, oauth
├── crates/engine/         ← session, registre de commandes (partagé par l'UI et le MCP), règles
├── crates/compose/        ← éditeur sur WordCraft doc/layout, export HTML
├── crates/antidote/       ← pont WebSocket local vers Antidote
├── crates/safety/         ← pièces jointes (type réel, approbation), liens, alertes
├── crates/porting/        ← import et export : eml, mbox, PDF, PST, eM Client
├── crates/ui/             ← interface egui
├── docs/                  ← guides OAuth, domaines contactés
└── .github/workflows/     ← build et tests (actions épinglées par SHA)
```

- Cible : Windows x64 (MSI et zip portable). Le code reste portable pour ajouter Linux plus tard (décision Q2).
- **Build** (décision Q3) : même processus que les autres applications de CraftTools.
  - Dépôt **public** `quackez-dev/mailalm`, réglé par `public-repo-settings.sh` (rulesets, actions épinglées par
    SHA, jeton en lecture, approbation des contributeurs externes).
  - Seulement `main` et les tags `crafttools-v*` ; commits avec l'adresse `noreply` uniquement.
  - Rien de compilé ni d'exécuté sur le PC : `cargo build`, `test` et `clippy` tournent sur Actions.
  - Artefacts gardés 1 jour, récupérés dans `builds/mailalm/<version>/`, sommes vérifiées, puis effacés.
- Audits : `audits/mailalm/` (code repris, nouvelles crates, revue de sécurité de chaque phase).

## 14. Fonctions supplémentaires retenues (décision Q13)

| ID | Fonction | Section |
|---|---|---|
| C1 | Annulation d'envoi et envoi différé | 8 |
| C2 | Report d'un message (« Snooze ») et rappel de suivi (« pas de réponse dans 3 jours ») | 14.1 |
| C3 | Boîte prioritaire synchronisée (Focused de Graph, catégories de Gmail) | 5 |
| C4 | Modèles, réponses rapides, signatures par compte | 8 |
| C5 | Règles locales et règles serveur dans le même éditeur, avec aperçu des messages touchés | 14.1 |
| C6 | Export `.eml`, `.mbox` et PDF ; import depuis eM Client, Outlook (PST) et Thunderbird (mbox) | 14.1 |
| C7 | Mode hors ligne complet, avec file d'attente des changements | 14.1 |
| S4 | Désabonnement en un clic, avec confirmation | 10.2 |
| A1 | Tâches synchronisées : Microsoft To Do et Google Tasks | 7 |
| A2 | Lien Teams pour un compte Hotmail, si l'API le permet | 7 |
| A3 | Disponibilités de tous les agendas de tous les comptes | 7 |
| K1 | Contacts synchronisés par compte | 4.1, 8 |
| M2 | Tri assisté par MCP, validé dans MailALM | 11 |

### 14.1 Détails

- **Snooze et suivi** (C2) : un message reporté disparaît de la boîte de réception et revient à l'heure choisie.
  Le rappel de suivi revient si aucune réponse n'est arrivée dans le fil. Côté serveur quand le fournisseur le
  permet, sinon local.
- **Règles** (C5) : un seul éditeur. Chaque règle indique où elle s'exécute : sur le serveur (Graph
  `messageRules`, filtres Gmail, Sieve) ou dans MailALM (quand il tourne). Aperçu des messages existants touchés
  avant l'enregistrement. Actions des règles : déplacer, marquer, catégorie, drapeau, supprimer. Jamais de
  lancement de programme ; le transfert automatique demande une confirmation explicite à la création.
- **Export et import** (C6) : l'import lit des fichiers non fiables (PST, mbox, base eM Client). Il applique les
  mêmes plafonds et les mêmes règles que la réception d'un courriel. Le lecteur PST de MailCraft est un candidat à
  la reprise (audité avant usage). Le format de la base eM Client est à étudier.
- **Hors ligne** (C7) : lecture, recherche, rédaction, classement, agenda et tâches fonctionnent sans réseau. Les
  changements partent à la reconnexion, dans l'ordre. Un conflit est montré à l'utilisateur.

## 15. Décisions (2026-11-22)

| # | Question | Décision |
|---|---|---|
| Q1 | Base de code | Nouvelle application qui reprend des briques de MailCraft et de WordCraft, **auditées avant usage** |
| Q2 | Plateformes | Windows d'abord, code portable |
| Q3 | Build et tests | Même processus que les autres applications : build sur GitHub Actions, dépôt public ; audit de tout code repris |
| Q4 | Dépôt GitHub | `quackez-dev/mailalm`, **public** |
| Q5 | Identifiants OAuth | Une application Azure et un projet Google créés par l'utilisateur ; identifiants compilés par le CI depuis des secrets |
| Q6 | Blocage Gmail | Filtre vers Spam si Gmail l'accepte, sinon Corbeille |
| Q7 | Rendu HTML | Sans moteur web |
| Q8 | Base locale | Non chiffrée |
| Q9 | Antidote | Antidote 12 ; je rédige la demande à Druide, l'utilisateur l'envoie |
| Q10 | Vue unifiée | En option, en plus des comptes séparés |
| Q11 | Langue | Anglais par défaut, français en option |
| Q12 | Droits MCP | Tout, avec confirmation dans MailALM des actions sensibles ; réglable par compte |
| Q13 | Suggestions | Retenues : C1 à C7, S4, A1, A2, A3, K1, M2. Les autres vont dans [IDEES.md](IDEES.md) |
| Q14 | Domaines | `@hotmail.com`, `@outlook.com`, `@live.ca`, `@bell.net`, `@videotron.ca` (et `@gmail.com`) |

## 16. Étapes

| Phase | Contenu | Livrable |
|---|---|---|
| 0 | Plan validé ; design Claude Design validé ; demande d'API envoyée à Druide | ce fichier, maquettes |
| 1 | Squelette : espace de travail, dépôt public, CI, audit des dépendances et du code repris, fenêtre au design | premier build Actions |
| 2 | Comptes IMAP/SMTP (Vidéotron, Bell) : lecture, envoi, dossiers, cache, recherche locale, hors ligne | Vidéotron et Bell utilisables |
| 3 | Hotmail par Graph : courriel, blocage serveur, règles, catégories, boîte prioritaire | Hotmail utilisable |
| 4 | Gmail par API : courriel, filtres, spam, libellés, catégories | Gmail utilisable |
| 5 | Pièces jointes : approbation, aperçu interne, envoi | |
| 6 | Agenda (Google, Graph, local), rappels, invitations, Meet, Teams, disponibilités globales, tâches | |
| 7 | Rédaction riche (WordCraft), modèles, signatures, annulation d'envoi, envoi différé, Antidote | |
| 8 | Contacts, recherche multi-comptes avancée, règles unifiées, snooze et suivi, désabonnement | |
| 9 | Serveur MCP et tri assisté | |
| 10 | Export et import (eml, mbox, PDF, PST, eM Client), installeur MSI | version 1.0 |

À chaque phase : revue de sécurité du code ajouté, audit des nouvelles dépendances et du code repris, build et
tests sur Actions.

## Sources

- Microsoft Graph, `message: markAsJunk` : https://learn.microsoft.com/en-us/graph/api/message-markasjunk?view=graph-rest-1.0
- Microsoft Graph, `message: markAsNotJunk` : https://learn.microsoft.com/en-us/graph/api/message-markasnotjunk?view=graph-rest-1.0
- Outlook.com, expéditeurs bloqués et approuvés : https://support.microsoft.com/outlook/block-or-allow-junk-email-settings
- API Gmail, filtres : https://developers.google.com/workspace/gmail/api/guides/filter_settings
- Antidote, outils pour développeurs : https://antidote.info/en/antidote-10/documentation/developer-tools
- Antidote, intégration dans un éditeur tiers : https://antidote.info/en/help-centre/faq/questions/editor
- Audits CraftTools : `audits/mailcraft/`, `audits/wordcraft/`
