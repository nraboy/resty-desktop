import React from "react";

interface InputProps extends React.InputHTMLAttributes<HTMLInputElement> {
  label?: string;
  error?: string;
}

export default function Input({ label, error, className = "", ...props }: InputProps) {
  return (
    <div className="flex flex-col gap-1">
      {label && (
        <label className="text-sm text-gray-400 font-medium">{label}</label>
      )}
      <input
        {...props}
        className={`bg-gray-800 border ${
          error ? "border-red-500" : "border-gray-700"
        } text-gray-100 rounded-md px-3 py-2 text-sm
          placeholder-gray-500 focus:outline-none focus:ring-2 focus:ring-blue-500
          disabled:opacity-50 ${className}`}
      />
      {error && <p className="text-xs text-red-400">{error}</p>}
    </div>
  );
}
