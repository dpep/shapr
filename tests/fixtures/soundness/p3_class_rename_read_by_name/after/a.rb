class Gadget
  def label = self.class.name
end

puts Gadget.new.label
