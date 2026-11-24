class Cart
  def total(items)
    subtotal = 0
    items.each { |item| subtotal += item.price }
    subtotal
  end
end
