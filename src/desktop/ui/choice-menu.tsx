import { Button } from "@nessa-ui/react/button"
import { Fragment } from "react"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuLabel,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
} from "./menu"

/** One choice in a `ChoiceMenu`. Options sharing a `section` are listed under it. */
export type ChoiceOption<T extends string> = {
  id: T
  label: string
  /** Shown but not choosable, such as a place not connected yet. */
  disabled?: boolean
  section?: string
}

const chevron = (
  <svg
    viewBox="0 0 16 16"
    width="12"
    height="12"
    aria-hidden="true"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.8"
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <path d="M4 6l4 4 4-4" />
  </svg>
)

/**
 * One of a few choices, picked from a menu: a button that says the current
 * choice and opens the rest. For a row's control where the choices would not
 * fit side by side, or where more may be listed (each connected computer).
 * The button is named by `label` and says the choice; items carry `data-value`.
 */
export function ChoiceMenu<T extends string>({
  value,
  options,
  onChange,
  label,
  disabled,
  describedBy,
  placeholder = "Choose",
  className,
  ...data
}: {
  value: T | undefined
  options: readonly ChoiceOption<T>[]
  onChange: (next: T) => void
  label: string
  disabled?: boolean
  describedBy?: string
  placeholder?: string
  className?: string
} & { [data: `data-${string}`]: string | boolean | undefined }) {
  const current = options.find((option) => option.id === value)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          {...data}
          variant="secondary"
          size="sm"
          className={className ? `choice-menu ${className}` : "choice-menu"}
          disabled={disabled}
          aria-label={`${label}: ${current?.label ?? placeholder}`}
          aria-describedby={describedBy}
          data-choice-trigger=""
          data-choice={value ?? ""}
        >
          <span className="choice-menu-value">{current?.label ?? placeholder}</span>
          {chevron}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" sideOffset={6}>
        <MenuRadioGroup
          value={value ?? ""}
          onValueChange={(next) => {
            const option = options.find((each) => each.id === next)
            if (option && !option.disabled && option.id !== value) onChange(option.id)
          }}
        >
          {options.map((option, index) => {
            const opens = option.section && option.section !== options[index - 1]?.section
            return (
              <Fragment key={option.id}>
                {opens ? (
                  <>
                    {index > 0 ? <MenuSeparator /> : null}
                    <MenuLabel>{option.section}</MenuLabel>
                  </>
                ) : null}
                <MenuRadioItem
                  value={option.id}
                  data-value={option.id}
                  disabled={option.disabled === true}
                >
                  {option.label}
                </MenuRadioItem>
              </Fragment>
            )
          })}
        </MenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
