# frozen_string_literal: true

module Zoo
  class Receipt
    def print(order)
      "total: #{order.total}"
    end
  end

  DEFAULT_RECEIPT = Receipt.new
end
