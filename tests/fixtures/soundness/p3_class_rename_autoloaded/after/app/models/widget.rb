class Gadget
  def label
    "w"
  end
end

class Order
  def widget
    Gadget.new
  end
end
