# frozen_string_literal: true

module Zoo
  class Order
    def total
      42
    end
  end

  class Receipt
    def print(order)
      "total: #{order.total}"
    end
  end
end
