# frozen_string_literal: false

module Zoo
  # Renders money and receipts.
  class Formatter
    def initialize(currency = 'USD')
      @currency = currency
    end

    def format_money(amount)
      format('%.2f %s', amount, @currency)
    end

    def render_line(item)
      name = item.name
      "#{name}: #{format_money(item.subtotal)}"
    end

    def receipt(items)
      lines = [ ]
      items.each do |item|
            lines << render_line(item)
      end
      lines.join("\n")
    end
  end
end
