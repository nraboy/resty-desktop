import React from "react";
import { XIcon } from "./icons";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
  onClear?: () => void;
}

const Input = React.forwardRef<HTMLInputElement, InputProps>(
  ({ label, error, onClear, className = "", ...props }, ref) => {
    const showClear = onClear && props.value !== "" && props.value !== undefined;

    return (
      <div className={`flex flex-col gap-1 ${className}`}>
        {label && (
          <label className="text-sm text-gray-400 font-medium">{label}</label>
        )}
        <div className="relative">
          <input
            ref={ref}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            {...props}
            className={`w-full bg-gray-800 border ${
              error ? "border-red-700" : "border-gray-700"
            } text-gray-100 rounded-md px-3 py-2 text-sm
              placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500
              disabled:opacity-50 ${showClear ? "pr-8" : ""}`}
          />
          {showClear && (
            <button
              type="button"
              onClick={onClear}
              tabIndex={-1}
              className="absolute inset-y-0 right-2 flex items-center text-gray-500 hover:text-gray-300 transition-colors"
            >
              <XIcon className="w-3.5 h-3.5" />
            </button>
          )}
        </div>
        {error && <p className="text-xs text-red-300">{error}</p>}
      </div>
    );
  }
);

Input.displayName = "Input";

export default Input;
