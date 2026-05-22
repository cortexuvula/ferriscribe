export interface LetterAudience {
  id: string;
  name: string;
  system_prompt: string;
  user_template: string | null;
  is_builtin: boolean;
  created_at: string;
  updated_at: string;
}
